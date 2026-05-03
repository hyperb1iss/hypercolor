//! Ableton Push 2 MIDI + display protocol.

mod display;
mod led_palette;

use std::borrow::Cow;
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceTopologyHint, DisplayFrameFormat,
    DisplayFramePayload,
};
use tracing::warn;

use crate::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolKeepalive, ProtocolResponse, ProtocolZone,
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
const PUSH2_DISPLAY_TRANSFER_CHUNK: usize = 16 * 1024;
const PUSH2_DISPLAY_LINE_PIXELS: usize = PUSH2_DISPLAY_WIDTH * 2;
const PUSH2_DISPLAY_LINE_PADDING: usize = 128;
const PUSH2_DISPLAY_LINE_SIZE: usize = PUSH2_DISPLAY_LINE_PIXELS + PUSH2_DISPLAY_LINE_PADDING;
const PUSH2_DEFAULT_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const PUSH2_RESYNC_INTERVAL: Duration = Duration::from_secs(5);
const PUSH2_IDENTITY_REQUEST: [u8; 6] = [0xF0, 0x7E, 0x01, 0x06, 0x01, 0xF7];
const PUSH2_MANUFACTURER_PREFIX: [u8; 6] = [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01];
const PUSH2_DISPLAY_XOR_MASK: [u8; 4] = [0xE7, 0xF3, 0xE7, 0xFF];
const PUSH2_REAPPLY_PALETTE_MESSAGE: [u8; 8] = [
    0xF0,
    0x00,
    0x21,
    0x1D,
    0x01,
    0x01,
    PUSH2_CMD_REAPPLY_PALETTE,
    0xF7,
];
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
    PUSH2_DISPLAY_TRANSFER_CHUNK.is_multiple_of(PUSH2_DISPLAY_PACKET_SIZE),
    "Push2 display transfer chunk size must align to the USB packet size"
);

#[derive(Debug)]
struct Push2State {
    palette: [[u8; 4]; PUSH2_PALETTE_SIZE],
    factory_palette: [[u8; 4]; PUSH2_PALETTE_SIZE],
    factory_palette_valid: [bool; PUSH2_PALETTE_SIZE],
    prev_led_indices: [u8; PUSH2_MIDI_LED_COUNT],
    prev_touch_strip: [u8; PUSH2_TOUCH_STRIP_LED_COUNT],
    last_colors: [[u8; 3]; PUSH2_TOTAL_LEDS],
    last_frame_seen: bool,
}

impl Default for Push2State {
    fn default() -> Self {
        Self {
            palette: [[0; 4]; PUSH2_PALETTE_SIZE],
            factory_palette: [[0; 4]; PUSH2_PALETTE_SIZE],
            factory_palette_valid: [false; PUSH2_PALETTE_SIZE],
            prev_led_indices: [0; PUSH2_MIDI_LED_COUNT],
            prev_touch_strip: [0; PUSH2_TOUCH_STRIP_LED_COUNT],
            last_colors: [[0; 3]; PUSH2_TOTAL_LEDS],
            last_frame_seen: false,
        }
    }
}

/// Palette-indexed Push 2 protocol implementation.
pub struct Push2Protocol {
    state: RwLock<Push2State>,
    display_encoder: Mutex<display::Push2DisplayEncoder>,
}

impl Push2Protocol {
    /// Create a new Push 2 protocol instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: RwLock::new(Push2State::default()),
            display_encoder: Mutex::new(display::Push2DisplayEncoder::default()),
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
        state.last_colors = [[0; 3]; PUSH2_TOTAL_LEDS];
        state.last_frame_seen = false;
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
        commands.extend(led_palette::all_leds_off_commands());
        commands
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        let mut commands = led_palette::all_leds_off_commands();
        let mut state = self
            .state
            .write()
            .expect("Push 2 state lock should not be poisoned");
        commands.extend(led_palette::restore_factory_palette_commands(&mut state));
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

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let normalized = self.normalize_colors(colors);
        let normalized = normalized.as_ref();
        let mut state = self
            .state
            .write()
            .expect("Push 2 state lock should not be poisoned");
        state.last_colors.copy_from_slice(normalized);
        state.last_frame_seen = true;
        led_palette::encode_led_frame(&mut state, normalized, commands, false);
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

    fn keepalive(&self) -> Option<ProtocolKeepalive> {
        Some(ProtocolKeepalive {
            commands: vec![
                primary_command(
                    push2_sysex(PUSH2_CMD_SET_MIDI_MODE, &[PUSH2_MIDI_MODE_USER]),
                    false,
                ),
                primary_command(
                    push2_sysex(
                        PUSH2_CMD_SET_TOUCH_STRIP_CONFIG,
                        &[PUSH2_TOUCH_STRIP_HOST_CONFIG],
                    ),
                    false,
                ),
            ],
            interval: PUSH2_RESYNC_INTERVAL,
        })
    }

    fn keepalive_commands(&self) -> Vec<ProtocolCommand> {
        let mut commands = self
            .keepalive()
            .map_or_else(Vec::new, |keepalive| keepalive.commands);
        let mut state = self
            .state
            .write()
            .expect("Push 2 state lock should not be poisoned");

        if !state.last_frame_seen {
            return commands;
        }

        let last_colors = state.last_colors;
        state.prev_led_indices = [0; PUSH2_MIDI_LED_COUNT];
        state.prev_touch_strip = [0; PUSH2_TOUCH_STRIP_LED_COUNT];

        let mut frame_commands = Vec::new();
        led_palette::encode_led_frame(&mut state, &last_colors, &mut frame_commands, true);
        commands.extend(frame_commands);
        commands
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

        if data.len() < 8 || data[1..6] != [0x00, 0x21, 0x1D, 0x01, 0x01] {
            return Err(ProtocolError::MalformedResponse {
                detail: "response did not include the Push 2 manufacturer header".to_owned(),
            });
        }

        let command = data[6];
        let args = &data[7..data.len() - 1];
        if command == PUSH2_CMD_GET_PALETTE_ENTRY {
            let mut state = self
                .state
                .write()
                .expect("Push 2 state lock should not be poisoned");
            led_palette::parse_palette_entry_response(args, &mut state)?;
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
        self.display_encoder
            .lock()
            .expect("Push 2 display encoder lock should not be poisoned")
            .encode_display_frame_from_jpeg(jpeg_data, commands)
    }

    fn encode_display_payload_into(
        &self,
        payload: DisplayFramePayload<'_>,
        commands: &mut Vec<ProtocolCommand>,
    ) -> Option<()> {
        let mut encoder = self
            .display_encoder
            .lock()
            .expect("Push 2 display encoder lock should not be poisoned");
        match payload.format {
            DisplayFrameFormat::Jpeg => {
                encoder.encode_display_frame_from_jpeg(payload.data, commands)
            }
            DisplayFrameFormat::Rgb => encoder.encode_display_frame_from_rgb(
                payload.width,
                payload.height,
                payload.data,
                commands,
            ),
        }
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        vec![
            ProtocolZone {
                name: "Pads".to_owned(),
                led_count: 64,
                topology: DeviceTopologyHint::Matrix { rows: 8, cols: 8 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ProtocolZone {
                name: "Buttons Above".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ProtocolZone {
                name: "Buttons Below".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ProtocolZone {
                name: "Scene Launch".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ProtocolZone {
                name: "Transport".to_owned(),
                led_count: 4,
                topology: DeviceTopologyHint::Custom,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ProtocolZone {
                name: "White Buttons".to_owned(),
                led_count: u32::try_from(PUSH2_WHITE_BUTTON_COUNT).unwrap_or(u32::MAX),
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ProtocolZone {
                name: "Touch Strip".to_owned(),
                led_count: 31,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ProtocolZone {
                name: "Display".to_owned(),
                led_count: 0,
                topology: DeviceTopologyHint::Display {
                    width: 960,
                    height: 160,
                    circular: false,
                },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
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
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
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

fn primary_command_slice(data: &[u8], expects_response: bool) -> ProtocolCommand {
    ProtocolCommand {
        data: data.to_vec(),
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

fn set_palette_entry_message(index: u8, entry: [u8; 4]) -> [u8; 17] {
    let mut message = [0_u8; 17];
    message[..7].copy_from_slice(&[
        0xF0,
        0x00,
        0x21,
        0x1D,
        0x01,
        0x01,
        PUSH2_CMD_SET_PALETTE_ENTRY,
    ]);
    message[7] = index;
    for (channel_index, value) in entry.iter().copied().enumerate() {
        let (lsb, msb) = encode_sysex_byte(value);
        let arg_offset = 8 + channel_index * 2;
        message[arg_offset] = lsb;
        message[arg_offset + 1] = msb;
    }
    message[16] = 0xF7;
    message
}
