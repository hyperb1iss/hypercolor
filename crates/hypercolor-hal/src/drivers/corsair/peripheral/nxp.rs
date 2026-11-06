//! Corsair NXP/CUE RGB packet encoder.
//!
//! Every command is one 64-byte output report on the vendor lighting
//! collection. The encoder never asks for a reply: on a hidraw-backed
//! transport the kernel carries an output report over the interface's
//! interrupt OUT endpoint when it has one and over a HID `SET_REPORT`
//! control request otherwise, which covers both firmware generations, but
//! the older generation answers through feature reports instead of input
//! reports, so waiting for an input report would stall it.

use std::borrow::Cow;
use std::cmp::min;
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceFeatures, DeviceTopologyHint,
    SegmentInfo,
};
use tracing::warn;
use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ResponseStatus,
    TransferType,
};

use super::types::{
    NXP_FULL_RANGE_PLANE_LEN, NXP_KEYBOARD_LED_SLOTS, NXP_PACKET_SIZE, NXP_PAYLOAD_LEN,
    NXP_RESPONSE_TIMEOUT, NxpColorSelector, NxpCommand, NxpDeviceConfig, NxpField, NxpLightingKind,
    NxpLightingMode,
};

/// Bulk chunks of one full-range color plane: `(chunk index, declared length)`.
const FULL_RANGE_CHUNKS: [(u8, usize); 3] = [(0x01, 60), (0x02, 60), (0x03, 48)];
/// Bulk chunks of the packed 512-color frame: `(chunk index, declared length)`.
const PACKED_512_CHUNKS: [(u8, usize); 4] = [(0x01, 60), (0x02, 60), (0x03, 60), (0x04, 36)];
/// Bytes in one packed 512-color plane (two LEDs per byte).
const PACKED_512_PLANE_LEN: usize = NXP_KEYBOARD_LED_SLOTS / 2;
/// Length check byte of the packed 512-color apply packet (three planes).
const PACKED_512_APPLY_LEN: u8 = 0xD8;
/// Chunk count in a full-range component commit.
const FULL_RANGE_CHUNK_COUNT: u8 = 0x03;
/// Zone entries that fit one mouse color packet (`id, R, G, B` each).
const MOUSE_PACKET_MAX_ZONES: usize = NXP_PAYLOAD_LEN / 4;
/// RGB triples that fit one mousepad color packet.
const MOUSEPAD_PACKET_MAX_ZONES: usize = NXP_PAYLOAD_LEN / 3;
/// Zones in the K55 zoned keyboard packet.
const ZONED_KEYBOARD_ZONES: usize = 3;
/// Zone name for a keyboard light bar.
const LIGHT_BAR_ZONE_NAME: &str = "Light Bar";

const _: () = assert!(
    FULL_RANGE_CHUNKS[0].1 + FULL_RANGE_CHUNKS[1].1 + FULL_RANGE_CHUNKS[2].1
        == NXP_FULL_RANGE_PLANE_LEN,
    "full-range chunks must cover the declared plane"
);

const _: () = assert!(
    std::mem::size_of::<NxpPacket>() == NXP_PACKET_SIZE,
    "NxpPacket must match NXP_PACKET_SIZE (64 bytes)"
);

/// Wire-format NXP/CUE command (64 bytes, no report ID byte).
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct NxpPacket {
    /// `SET`, `GET`, `WRITE_BULK`, or `READ_BULK`.
    command: u8,
    /// Protocol field, or the chunk index of a bulk write.
    field: u8,
    /// Command arguments (mode, color selector, declared length, zone count).
    args: [u8; 2],
    /// Command payload, zero-padded.
    payload: [u8; NXP_PAYLOAD_LEN],
}

impl NxpPacket {
    fn new(command: NxpCommand, field: u8, args: [u8; 2]) -> Self {
        let mut packet = Self::new_zeroed();
        packet.command = command as u8;
        packet.field = field;
        packet.args = args;
        packet
    }

    fn set(field: NxpField, args: [u8; 2]) -> Self {
        Self::new(NxpCommand::Set, field as u8, args)
    }
}

#[derive(Debug, Clone)]
pub struct CorsairNxpProtocol {
    config: NxpDeviceConfig,
}

impl CorsairNxpProtocol {
    #[must_use]
    pub const fn new(config: NxpDeviceConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub const fn config(&self) -> NxpDeviceConfig {
        self.config
    }

    fn normalize_colors<'a>(&self, colors: &'a [[u8; 3]]) -> Cow<'a, [[u8; 3]]> {
        let expected = self.config.led_count;
        if expected == 0 {
            return Cow::Borrowed(&[]);
        }

        if colors.len() == expected {
            return Cow::Borrowed(colors);
        }

        let mut normalized = vec![[0_u8; 3]; expected];
        let copy_len = min(colors.len(), expected);
        normalized[..copy_len].copy_from_slice(&colors[..copy_len]);

        warn!(
            expected,
            actual = colors.len(),
            device = self.config.name,
            "corsair nxp frame length mismatch; applying truncate/pad"
        );

        Cow::Owned(normalized)
    }

    fn push(commands: &mut CommandBuffer<'_>, packet: &NxpPacket) {
        commands.push_struct(
            packet,
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
    }

    fn lighting_mode_command(mode: NxpLightingMode) -> ProtocolCommand {
        let packet = NxpPacket::set(NxpField::Lighting, [mode as u8, 0x00]);
        ProtocolCommand {
            data: packet.as_bytes().to_vec(),
            ..ProtocolCommand::default()
        }
    }

    /// Color of one full-range plane slot. Key slots map straight to frame
    /// positions; light bar slots map to the bar positions that follow them.
    fn plane_slot_color(&self, colors: &[[u8; 3]], slot: usize) -> Option<[u8; 3]> {
        let key_slots = self.config.key_slots();
        if slot < key_slots {
            return colors.get(slot).copied();
        }
        let position = self
            .config
            .light_bar
            .iter()
            .position(|&bar_slot| usize::from(bar_slot) == slot)?;
        colors.get(key_slots + position).copied()
    }

    fn append_full_range_keyboard(&self, colors: &[[u8; 3]], commands: &mut CommandBuffer<'_>) {
        for (component, selector) in [
            (0_usize, NxpColorSelector::Red),
            (1, NxpColorSelector::Green),
            (2, NxpColorSelector::Blue),
        ] {
            append_plane_chunks(commands, &FULL_RANGE_CHUNKS, |slot| {
                self.plane_slot_color(colors, slot)
                    .map_or(0, |color| color[component])
            });
            Self::push(commands, &component_commit(selector));
        }
    }

    fn append_monochrome_keyboard(&self, colors: &[[u8; 3]], commands: &mut CommandBuffer<'_>) {
        append_plane_chunks(commands, &FULL_RANGE_CHUNKS, |slot| {
            self.plane_slot_color(colors, slot)
                .map_or(0, |color| color[0].max(color[1]).max(color[2]))
        });
        Self::push(commands, &component_commit(NxpColorSelector::Red));
    }

    fn append_packed_keyboard(colors: &[[u8; 3]], commands: &mut CommandBuffer<'_>) {
        append_plane_chunks(commands, &PACKED_512_CHUNKS, |offset| {
            packed_512_byte(colors, offset)
        });

        let mut apply = NxpPacket::set(NxpField::KeyboardPackedColor, [0x00, 0x00]);
        apply.payload[0] = PACKED_512_APPLY_LEN;
        Self::push(commands, &apply);
    }

    fn append_zoned_keyboard(colors: &[[u8; 3]], commands: &mut CommandBuffer<'_>) {
        let mut packet = NxpPacket::set(NxpField::KeyboardZoneColor, [0x00, 0x00]);
        for (index, color) in colors.iter().take(ZONED_KEYBOARD_ZONES).enumerate() {
            let offset = index * 3;
            packet.payload[offset..offset + 3].copy_from_slice(color);
        }
        Self::push(commands, &packet);
    }

    fn append_mouse(colors: &[[u8; 3]], commands: &mut CommandBuffer<'_>) {
        let zone_count = colors.len().min(MOUSE_PACKET_MAX_ZONES);
        let mut packet = NxpPacket::set(
            NxpField::MouseColor,
            [u8::try_from(zone_count).unwrap_or(u8::MAX), 0x01],
        );
        for (index, color) in colors.iter().take(zone_count).enumerate() {
            let offset = index * 4;
            packet.payload[offset] = u8::try_from(index + 1).unwrap_or(u8::MAX);
            packet.payload[offset + 1..offset + 4].copy_from_slice(color);
        }
        Self::push(commands, &packet);
    }

    fn append_mousepad(colors: &[[u8; 3]], commands: &mut CommandBuffer<'_>) {
        let zone_count = colors.len().min(MOUSEPAD_PACKET_MAX_ZONES);
        let mut packet = NxpPacket::set(
            NxpField::MouseColor,
            [u8::try_from(zone_count).unwrap_or(u8::MAX), 0x00],
        );
        for (index, color) in colors.iter().take(zone_count).enumerate() {
            let offset = index * 3;
            packet.payload[offset..offset + 3].copy_from_slice(color);
        }
        Self::push(commands, &packet);
    }
}

impl Protocol for CorsairNxpProtocol {
    fn name(&self) -> &'static str {
        self.config.name
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        if !self.config.supports_direct() {
            return Vec::new();
        }

        vec![Self::lighting_mode_command(NxpLightingMode::Software)]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        if self.config.requires_unclean_exit || !self.config.supports_direct() {
            return Vec::new();
        }

        vec![Self::lighting_mode_command(NxpLightingMode::Hardware)]
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        if !self.config.supports_direct() {
            commands.clear();
            return;
        }

        let normalized = self.normalize_colors(colors);
        let colors = normalized.as_ref();
        let mut buffer = CommandBuffer::new(commands);
        match self.config.lighting_kind {
            NxpLightingKind::FullRangeKeyboard => {
                self.append_full_range_keyboard(colors, &mut buffer);
            }
            NxpLightingKind::Packed512Keyboard => Self::append_packed_keyboard(colors, &mut buffer),
            NxpLightingKind::ZonedKeyboard => Self::append_zoned_keyboard(colors, &mut buffer),
            NxpLightingKind::MonochromeKeyboard => {
                self.append_monochrome_keyboard(colors, &mut buffer);
            }
            NxpLightingKind::Mouse => Self::append_mouse(colors, &mut buffer),
            NxpLightingKind::Mousepad => Self::append_mousepad(colors, &mut buffer),
            NxpLightingKind::NoLights => {}
        }
        buffer.finish();
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        if data.is_empty() {
            return Err(ProtocolError::MalformedResponse {
                detail: "empty Corsair NXP response".to_owned(),
            });
        }

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data.to_vec(),
        })
    }

    fn response_timeout(&self) -> Duration {
        NXP_RESPONSE_TIMEOUT
    }

    fn zones(&self) -> Vec<SegmentInfo> {
        if !self.config.supports_direct() {
            return Vec::new();
        }

        let mut zones = vec![SegmentInfo {
            name: self.config.class.zone_name().to_owned(),
            led_count: u32::try_from(self.config.key_slots()).unwrap_or(u32::MAX),
            topology: self.config.topology.hint(),
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }];
        if !self.config.light_bar.is_empty() {
            let bar_leds = u32::try_from(self.config.light_bar.len()).unwrap_or(u32::MAX);
            zones.push(SegmentInfo {
                name: LIGHT_BAR_ZONE_NAME.to_owned(),
                led_count: bar_leds,
                topology: DeviceTopologyHint::Matrix {
                    rows: 1,
                    cols: bar_leds,
                },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            });
        }
        zones
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: self.config.supports_direct(),
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: self.config.max_fps,
            color_space: DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        u32::try_from(self.config.led_count).unwrap_or(u32::MAX)
    }

    fn frame_interval(&self) -> Duration {
        self.config.frame_interval()
    }
}

/// Emit one `WRITE_BULK` packet per declared chunk of a byte plane.
///
/// `byte_at` yields the plane byte at an absolute offset. Each chunk
/// starts where the previous chunk's declared length ended and copies at
/// most one payload's worth of bytes; the rest of the packet stays zero.
fn append_plane_chunks<F>(commands: &mut CommandBuffer<'_>, chunks: &[(u8, usize)], byte_at: F)
where
    F: Fn(usize) -> u8,
{
    let mut cursor = 0_usize;
    for &(index, declared_len) in chunks {
        let mut packet = NxpPacket::new(
            NxpCommand::WriteBulk,
            index,
            [u8::try_from(declared_len).unwrap_or(u8::MAX), 0x00],
        );
        let copy_len = declared_len.min(NXP_PAYLOAD_LEN);
        for (slot, byte) in packet.payload[..copy_len].iter_mut().enumerate() {
            *byte = byte_at(cursor + slot);
        }
        CorsairNxpProtocol::push(commands, &packet);
        cursor = cursor.saturating_add(declared_len);
    }
}

fn component_commit(selector: NxpColorSelector) -> NxpPacket {
    let mut packet = NxpPacket::set(
        NxpField::KeyboardColor,
        [selector as u8, FULL_RANGE_CHUNK_COUNT],
    );
    packet.payload[0] = if selector == NxpColorSelector::Blue {
        0x02
    } else {
        0x01
    };
    packet
}

/// Byte at `offset` of the packed 512-color payload: red, green, then blue
/// planes of [`PACKED_512_PLANE_LEN`] bytes, two LEDs per byte with each
/// component truncated to three bits and inverted.
fn packed_512_byte(colors: &[[u8; 3]], offset: usize) -> u8 {
    let component = offset / PACKED_512_PLANE_LEN;
    if component > 2 {
        return 0;
    }
    let pair = offset % PACKED_512_PLANE_LEN;
    let level = |index: usize| colors.get(index).map_or(0, |color| color[component] >> 5);
    let first = level(pair * 2);
    let second = level(pair * 2 + 1);
    ((7 - second) << 4) | (7 - first)
}
