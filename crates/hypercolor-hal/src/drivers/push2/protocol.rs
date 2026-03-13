//! Ableton Push 2 MIDI + display protocol.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceTopologyHint,
};
use image::{ImageFormat, imageops::FilterType};
use tracing::warn;
use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone,
    ResponseStatus, TransferType,
};

const PUSH2_RGB_LED_COUNT: usize = 92;
const PUSH2_WHITE_BUTTON_COUNT: usize = 37;
const PUSH2_MIDI_LED_COUNT: usize = PUSH2_RGB_LED_COUNT + PUSH2_WHITE_BUTTON_COUNT;
const PUSH2_TOTAL_LEDS: usize = PUSH2_MIDI_LED_COUNT + PUSH2_TOUCH_STRIP_LED_COUNT;
const PUSH2_PAD_COUNT: usize = 64;
const PUSH2_TOUCH_STRIP_LED_COUNT: usize = 31;
const PUSH2_RGB_BUTTON_COUNT: usize = 28;
const PUSH2_PALETTE_SIZE: usize = 128;
const PUSH2_RGB_SLOT_LIMIT: usize = 97;
const PUSH2_WHITE_SLOT_START: u8 = 97;
const PUSH2_WHITE_SLOT_COUNT: u8 = 31;
const PUSH2_DISPLAY_WIDTH: usize = 960;
const PUSH2_DISPLAY_HEIGHT: usize = 160;
const PUSH2_DISPLAY_PACKET_SIZE: usize = 512;
const PUSH2_DISPLAY_LINE_PIXELS: usize = PUSH2_DISPLAY_WIDTH * 2;
const PUSH2_DISPLAY_LINE_PADDING: usize = 128;
const PUSH2_DISPLAY_LINE_SIZE: usize = PUSH2_DISPLAY_LINE_PIXELS + PUSH2_DISPLAY_LINE_PADDING;
const PUSH2_DEFAULT_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const PUSH2_IDENTITY_REQUEST: [u8; 6] = [0xF0, 0x7E, 0x01, 0x06, 0x01, 0xF7];
const PUSH2_MANUFACTURER_PREFIX: [u8; 6] = [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01];
const PUSH2_DISPLAY_XOR_MASK: [u8; 4] = [0xE7, 0xF3, 0xE7, 0xFF];
const PUSH2_CMD_SET_PALETTE_ENTRY: u8 = 0x03;
const PUSH2_CMD_GET_PALETTE_ENTRY: u8 = 0x04;
const PUSH2_CMD_REAPPLY_PALETTE: u8 = 0x05;
const PUSH2_CMD_SET_LED_BRIGHTNESS: u8 = 0x06;
const PUSH2_CMD_SET_DISPLAY_BRIGHTNESS: u8 = 0x08;
const PUSH2_CMD_SET_MIDI_MODE: u8 = 0x0A;
const PUSH2_CMD_SET_TOUCH_STRIP_CONFIG: u8 = 0x17;
const PUSH2_CMD_SET_TOUCH_STRIP_LEDS: u8 = 0x19;
const PUSH2_CMD_REQUEST_STATS: u8 = 0x1A;
const PUSH2_MIDI_MODE_LIVE: u8 = 0x00;
const PUSH2_MIDI_MODE_USER: u8 = 0x01;
const PUSH2_TOUCH_STRIP_HOST_CONFIG: u8 = 0x6B;
const PUSH2_TOUCH_STRIP_DEFAULT_CONFIG: u8 = 0x68;

const PAD_NOTE_MAP: [u8; PUSH2_PAD_COUNT] = [
    36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59,
    60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83,
    84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99,
];

const RGB_BUTTON_CC_MAP: [u8; PUSH2_RGB_BUTTON_COUNT] = [
    102, 103, 104, 105, 106, 107, 108, 109, 20, 21, 22, 23, 24, 25, 26, 27, 43, 42, 41, 40, 39, 38,
    37, 36, 85, 86, 3, 9,
];

const WHITE_BUTTON_CC_MAP: [u8; PUSH2_WHITE_BUTTON_COUNT] = [
    28, 29, 30, 31, 35, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 59, 61, 62, 63, 87, 88, 89,
    90, 110, 111, 112, 113, 116, 117, 118, 119, 56, 57, 58, 60,
];

const _: () = assert!(
    std::mem::size_of::<Push2DisplayHeader>() == 16,
    "Push2DisplayHeader must be exactly 16 bytes"
);
const _: () = assert!(
    std::mem::size_of::<Push2DisplayLine>() == PUSH2_DISPLAY_LINE_SIZE,
    "Push2DisplayLine must be exactly 2048 bytes"
);

#[derive(IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct Push2DisplayHeader {
    magic: [u8; 4],
    padding: [u8; 12],
}

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct Push2DisplayLine {
    pixels: [u8; PUSH2_DISPLAY_LINE_PIXELS],
    padding: [u8; PUSH2_DISPLAY_LINE_PADDING],
}

const DISPLAY_HEADER: Push2DisplayHeader = Push2DisplayHeader {
    magic: [0xFF, 0xCC, 0xAA, 0x88],
    padding: [0; 12],
};

#[derive(Debug)]
struct Push2State {
    palette: [[u8; 4]; PUSH2_PALETTE_SIZE],
    factory_palette: [[u8; 4]; PUSH2_PALETTE_SIZE],
    factory_palette_valid: [bool; PUSH2_PALETTE_SIZE],
    prev_led_indices: [u8; PUSH2_MIDI_LED_COUNT],
    prev_touch_strip: [u8; PUSH2_TOUCH_STRIP_LED_COUNT],
}

impl Default for Push2State {
    fn default() -> Self {
        Self {
            palette: [[0; 4]; PUSH2_PALETTE_SIZE],
            factory_palette: [[0; 4]; PUSH2_PALETTE_SIZE],
            factory_palette_valid: [false; PUSH2_PALETTE_SIZE],
            prev_led_indices: [0; PUSH2_MIDI_LED_COUNT],
            prev_touch_strip: [0; PUSH2_TOUCH_STRIP_LED_COUNT],
        }
    }
}

/// Palette-indexed Push 2 protocol implementation.
pub struct Push2Protocol {
    state: RwLock<Push2State>,
}

impl Push2Protocol {
    /// Create a new Push 2 protocol instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: RwLock::new(Push2State::default()),
        }
    }

    #[expect(
        clippy::unused_self,
        reason = "method is called via self in Protocol trait impl"
    )]
    fn normalize_colors<'a>(&self, colors: &'a [[u8; 3]]) -> Cow<'a, [[u8; 3]]> {
        if colors.len() == PUSH2_TOTAL_LEDS {
            return Cow::Borrowed(colors);
        }

        let mut normalized = vec![[0_u8; 3]; PUSH2_TOTAL_LEDS];
        let copy_len = colors.len().min(PUSH2_TOTAL_LEDS);
        normalized[..copy_len].copy_from_slice(&colors[..copy_len]);

        warn!(
            expected = PUSH2_TOTAL_LEDS,
            actual = colors.len(),
            "push2 frame length mismatch; applying truncate/pad"
        );

        Cow::Owned(normalized)
    }

    fn restore_factory_palette_commands(state: &mut Push2State) -> Vec<ProtocolCommand> {
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

            commands.push(primary_command(
                set_palette_entry_message(u8::try_from(index).unwrap_or(u8::MAX), factory),
                false,
            ));
            state.palette[index] = factory;
            restored_any = true;
        }

        if restored_any {
            commands.push(primary_command(
                push2_sysex(PUSH2_CMD_REAPPLY_PALETTE, &[]),
                false,
            ));
        }

        commands
    }

    #[expect(
        clippy::unused_self,
        reason = "method is called via self in Protocol trait impl"
    )]
    fn build_display_commands(&self, rgb_bytes: &[u8], commands: &mut Vec<ProtocolCommand>) {
        let mut buffer = CommandBuffer::new(commands);
        buffer.push_struct(
            &DISPLAY_HEADER,
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Bulk,
        );

        for row in 0..PUSH2_DISPLAY_HEIGHT {
            let row_start = row * PUSH2_DISPLAY_WIDTH * 3;
            let mut line = Push2DisplayLine::new_zeroed();

            for column in 0..PUSH2_DISPLAY_WIDTH {
                let rgb_offset = row_start + column * 3;
                let pixel_offset = column * 2;
                let encoded = encode_rgb565(
                    rgb_bytes[rgb_offset],
                    rgb_bytes[rgb_offset + 1],
                    rgb_bytes[rgb_offset + 2],
                );
                line.pixels[pixel_offset..pixel_offset + 2].copy_from_slice(&encoded);
            }

            xor_shape_line(&mut line);

            for chunk in line.as_bytes().chunks(PUSH2_DISPLAY_PACKET_SIZE) {
                buffer.push_slice(
                    chunk,
                    false,
                    Duration::ZERO,
                    Duration::ZERO,
                    TransferType::Bulk,
                );
            }
        }

        buffer.finish();
    }
}

impl Default for Push2Protocol {
    fn default() -> Self {
        Self::new()
    }
}

impl Protocol for Push2Protocol {
    fn name(&self) -> &'static str {
        "Ableton Push 2"
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        let mut state = self
            .state
            .write()
            .expect("Push 2 state lock should not be poisoned");
        state.prev_led_indices = [0; PUSH2_MIDI_LED_COUNT];
        state.prev_touch_strip = [0; PUSH2_TOUCH_STRIP_LED_COUNT];
        drop(state);

        let mut commands = Vec::with_capacity(3 + PUSH2_PALETTE_SIZE + PUSH2_MIDI_LED_COUNT + 1);
        commands.push(ProtocolCommand {
            data: PUSH2_IDENTITY_REQUEST.to_vec(),
            expects_response: true,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        });
        commands.push(primary_command(
            push2_sysex(PUSH2_CMD_SET_MIDI_MODE, &[PUSH2_MIDI_MODE_USER]),
            true,
        ));
        commands.push(primary_command(
            push2_sysex(
                PUSH2_CMD_SET_TOUCH_STRIP_CONFIG,
                &[PUSH2_TOUCH_STRIP_HOST_CONFIG],
            ),
            false,
        ));
        for index in 0..PUSH2_PALETTE_SIZE {
            commands.push(primary_command(
                push2_sysex(
                    PUSH2_CMD_GET_PALETTE_ENTRY,
                    &[u8::try_from(index).unwrap_or(u8::MAX)],
                ),
                true,
            ));
        }
        commands.extend(all_leds_off_commands());
        commands
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        let mut state = self
            .state
            .write()
            .expect("Push 2 state lock should not be poisoned");
        let mut commands = Self::restore_factory_palette_commands(&mut state);
        state.prev_led_indices = [0; PUSH2_MIDI_LED_COUNT];
        state.prev_touch_strip = [0; PUSH2_TOUCH_STRIP_LED_COUNT];
        drop(state);

        commands.push(primary_command(
            push2_sysex(PUSH2_CMD_SET_MIDI_MODE, &[PUSH2_MIDI_MODE_LIVE]),
            true,
        ));
        commands.push(primary_command(
            push2_sysex(
                PUSH2_CMD_SET_TOUCH_STRIP_CONFIG,
                &[PUSH2_TOUCH_STRIP_DEFAULT_CONFIG],
            ),
            false,
        ));
        commands
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    #[expect(
        clippy::too_many_lines,
        reason = "push2 frame encoding has inherent per-zone complexity"
    )]
    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let normalized = self.normalize_colors(colors);
        let normalized = normalized.as_ref();
        let rgb_colors = &normalized[..PUSH2_RGB_LED_COUNT];
        let white_button_colors = &normalized[PUSH2_RGB_LED_COUNT..PUSH2_MIDI_LED_COUNT];
        let touch_strip_colors = &normalized[PUSH2_MIDI_LED_COUNT..];
        let mut state = self
            .state
            .write()
            .expect("Push 2 state lock should not be poisoned");
        let mut color_slots = HashMap::with_capacity(PUSH2_RGB_LED_COUNT);
        let mut assigned_slots = [false; PUSH2_PALETTE_SIZE];
        let mut white_button_slots = [0_u8; PUSH2_WHITE_BUTTON_COUNT];
        assigned_slots[0] = true;
        assigned_slots[PUSH2_RGB_SLOT_LIMIT..].fill(true);
        color_slots.insert([0, 0, 0], 0_u8);

        let mut command_buffer = CommandBuffer::new(commands);
        let mut palette_dirty = false;

        for color in rgb_colors {
            if color_slots.contains_key(color) {
                continue;
            }

            let entry = palette_entry(*color);
            let slot = if let Some(existing) = find_existing_slot(&state, entry, &assigned_slots) {
                existing
            } else {
                let free_slot = next_free_slot(&assigned_slots)
                    .expect("Push 2 RGB zones use at most 92 unique colors");
                if state.palette[usize::from(free_slot)] != entry {
                    command_buffer.push_slice(
                        &set_palette_entry_message(free_slot, entry),
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
                command_buffer.push_slice(
                    &set_palette_entry_message(slot, entry),
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
                &push2_sysex(PUSH2_CMD_REAPPLY_PALETTE, &[]),
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
            command_buffer.push_slice(
                &push2_sysex(PUSH2_CMD_SET_TOUCH_STRIP_LEDS, &packed),
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
            );
            state.prev_touch_strip = strip_levels;
        }

        command_buffer.finish();
    }

    #[expect(clippy::similar_names, reason = "lsb/msb are standard acronyms")]
    fn encode_brightness(&self, brightness: u8) -> Option<Vec<ProtocolCommand>> {
        let led_brightness = brightness / 2;
        let (display_lsb, display_msb) = encode_sysex_byte(brightness);
        Some(vec![
            primary_command(
                push2_sysex(PUSH2_CMD_SET_LED_BRIGHTNESS, &[led_brightness]),
                false,
            ),
            primary_command(
                push2_sysex(
                    PUSH2_CMD_SET_DISPLAY_BRIGHTNESS,
                    &[display_lsb, display_msb],
                ),
                false,
            ),
        ])
    }

    fn connection_diagnostics(&self) -> Vec<ProtocolCommand> {
        vec![primary_command(
            push2_sysex(PUSH2_CMD_REQUEST_STATS, &[]),
            true,
        )]
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        if data.len() < 2 || data.first() != Some(&0xF0) || data.last() != Some(&0xF7) {
            return Err(ProtocolError::MalformedResponse {
                detail: "response must be framed as SysEx".to_owned(),
            });
        }

        if data.len() >= 5 && data[1..5] == [0x7E, 0x01, 0x06, 0x02] {
            return Ok(ProtocolResponse {
                status: ResponseStatus::Ok,
                data: data[1..data.len() - 1].to_vec(),
            });
        }

        if data.len() < 8 || data[1..7] != [0x00, 0x21, 0x1D, 0x01, 0x01, data[6]] {
            return Err(ProtocolError::MalformedResponse {
                detail: "response did not include the Push 2 manufacturer header".to_owned(),
            });
        }

        let command = data[6];
        let args = &data[7..data.len() - 1];
        if command == PUSH2_CMD_GET_PALETTE_ENTRY {
            parse_palette_entry_response(args, &self.state)?;
        }

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data[6..data.len() - 1].to_vec(),
        })
    }

    fn encode_display_frame(&self, jpeg_data: &[u8]) -> Option<Vec<ProtocolCommand>> {
        let mut commands = Vec::new();
        self.encode_display_frame_into(jpeg_data, &mut commands)?;
        Some(commands)
    }

    fn encode_display_frame_into(
        &self,
        jpeg_data: &[u8],
        commands: &mut Vec<ProtocolCommand>,
    ) -> Option<()> {
        let image = image::load_from_memory_with_format(jpeg_data, ImageFormat::Jpeg).ok()?;
        let rgb = image
            .resize_exact(
                u32::try_from(PUSH2_DISPLAY_WIDTH).unwrap_or(960),
                u32::try_from(PUSH2_DISPLAY_HEIGHT).unwrap_or(160),
                FilterType::Nearest,
            )
            .to_rgb8();
        self.build_display_commands(rgb.as_raw(), commands);
        Some(())
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        vec![
            ProtocolZone {
                name: "Pads".to_owned(),
                led_count: 64,
                topology: DeviceTopologyHint::Matrix { rows: 8, cols: 8 },
                color_format: DeviceColorFormat::Rgb,
            },
            ProtocolZone {
                name: "Buttons Above".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            },
            ProtocolZone {
                name: "Buttons Below".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            },
            ProtocolZone {
                name: "Scene Launch".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            },
            ProtocolZone {
                name: "Transport".to_owned(),
                led_count: 4,
                topology: DeviceTopologyHint::Custom,
                color_format: DeviceColorFormat::Rgb,
            },
            ProtocolZone {
                name: "White Buttons".to_owned(),
                led_count: u32::try_from(PUSH2_WHITE_BUTTON_COUNT).unwrap_or(u32::MAX),
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            },
            ProtocolZone {
                name: "Touch Strip".to_owned(),
                led_count: 31,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            },
            ProtocolZone {
                name: "Display".to_owned(),
                led_count: 0,
                topology: DeviceTopologyHint::Display {
                    width: 960,
                    height: 160,
                    circular: false,
                },
                color_format: DeviceColorFormat::Jpeg,
            },
        ]
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: 160,
            supports_direct: true,
            supports_brightness: true,
            has_display: true,
            display_resolution: Some((960, 160)),
            max_fps: 60,
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        160
    }

    fn frame_interval(&self) -> Duration {
        PUSH2_DEFAULT_FRAME_INTERVAL
    }
}

fn primary_command(data: Vec<u8>, expects_response: bool) -> ProtocolCommand {
    ProtocolCommand {
        data,
        expects_response,
        response_delay: Duration::ZERO,
        post_delay: Duration::ZERO,
        transfer_type: TransferType::Primary,
    }
}

fn push2_sysex(command: u8, args: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(PUSH2_MANUFACTURER_PREFIX.len() + args.len() + 2);
    message.extend_from_slice(&PUSH2_MANUFACTURER_PREFIX);
    message.push(command);
    message.extend_from_slice(args);
    message.push(0xF7);
    message
}

fn encode_sysex_byte(value: u8) -> (u8, u8) {
    (value & 0x7F, (value >> 7) & 0x01)
}

fn decode_sysex_byte(lsb: u8, msb: u8) -> Result<u8, ProtocolError> {
    if lsb > 0x7F || msb > 0x01 {
        return Err(ProtocolError::MalformedResponse {
            detail: format!("invalid 7-bit encoded byte: lsb={lsb:#04X} msb={msb:#04X}"),
        });
    }

    Ok((msb << 7) | lsb)
}

fn derive_white_channel(rgb: [u8; 3]) -> u8 {
    let weighted =
        2_126_u32 * u32::from(rgb[0]) + 7_152_u32 * u32::from(rgb[1]) + 722_u32 * u32::from(rgb[2]);
    u8::try_from((weighted + 5_000) / 10_000).unwrap_or(u8::MAX)
}

fn palette_entry(rgb: [u8; 3]) -> [u8; 4] {
    [rgb[0], rgb[1], rgb[2], derive_white_channel(rgb)]
}

fn set_palette_entry_message(index: u8, entry: [u8; 4]) -> Vec<u8> {
    let mut args = [0_u8; 9];
    args[0] = index;
    for (channel_index, value) in entry.iter().copied().enumerate() {
        let (lsb, msb) = encode_sysex_byte(value);
        let arg_offset = 1 + channel_index * 2;
        args[arg_offset] = lsb;
        args[arg_offset + 1] = msb;
    }
    push2_sysex(PUSH2_CMD_SET_PALETTE_ENTRY, &args)
}

fn parse_palette_entry_response(
    args: &[u8],
    state: &RwLock<Push2State>,
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
    let mut entry = [0_u8; 4];
    for channel in 0..4 {
        entry[channel] = decode_sysex_byte(args[1 + channel * 2], args[2 + channel * 2])?;
    }

    let mut state = state
        .write()
        .expect("Push 2 state lock should not be poisoned");
    state.palette[index] = entry;
    state.factory_palette[index] = entry;
    state.factory_palette_valid[index] = true;
    Ok(())
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

fn all_leds_off_commands() -> Vec<ProtocolCommand> {
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
    commands.push(primary_command(
        push2_sysex(
            PUSH2_CMD_SET_TOUCH_STRIP_LEDS,
            &encode_touch_strip(&[0; PUSH2_TOUCH_STRIP_LED_COUNT]),
        ),
        false,
    ));
    commands
}

fn encode_rgb565(red: u8, green: u8, blue: u8) -> [u8; 2] {
    let encoded = (u16::from(blue >> 3) << 11) | (u16::from(green >> 2) << 5) | u16::from(red >> 3);
    encoded.to_le_bytes()
}

fn xor_shape_line(line: &mut Push2DisplayLine) {
    for (index, byte) in line.pixels.iter_mut().enumerate() {
        *byte ^= PUSH2_DISPLAY_XOR_MASK[index & 3];
    }
    for (index, byte) in line.padding.iter_mut().enumerate() {
        *byte ^= PUSH2_DISPLAY_XOR_MASK[(PUSH2_DISPLAY_LINE_PIXELS + index) & 3];
    }
}
