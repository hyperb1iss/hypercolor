//! Pure QMK HID RGB protocol encoder/decoder.
//!
//! Implements the QMK vendor HID protocol for per-key RGB control of
//! QMK-firmware keyboards. Supports protocol revisions 9, B/C, and D/E.

use std::borrow::Cow;
use std::cmp::min;
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceTopologyHint,
};
use tracing::warn;
use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone,
    ResponseStatus, TransferType,
};

use super::types::{Command, PACKET_SIZE, ProtocolRevision, SPEED_NORMAL, STATUS_FAILURE};

// ── Wire-format packets ──────────────────────────────────────────────────

/// 65-byte QMK HID RGB report.
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct QmkPacket {
    /// HID report ID (always 0x00).
    report_id: u8,
    /// Command byte.
    command: u8,
    /// Payload (up to 63 bytes).
    payload: [u8; 63],
}

const _: () = assert!(
    std::mem::size_of::<QmkPacket>() == PACKET_SIZE,
    "QmkPacket must be exactly 65 bytes"
);

// ── Protocol configuration ───────────────────────────────────────────────

/// Per-keyboard configuration for the QMK HID RGB protocol.
#[derive(Debug, Clone)]
pub struct QmkKeyboardConfig {
    /// Total addressable LEDs on this keyboard.
    pub led_count: usize,
    /// Protocol revision to use.
    pub revision: ProtocolRevision,
    /// Maximum LEDs per batch update (overridable for slower MCUs).
    pub leds_per_update: Option<usize>,
    /// Optional keyboard matrix dimensions (rows, cols) for zone topology.
    pub matrix: Option<(u32, u32)>,
    /// Whether the keyboard has an underglow zone.
    pub has_underglow: bool,
    /// Underglow LED count (only relevant when `has_underglow` is true).
    pub underglow_count: usize,
}

impl QmkKeyboardConfig {
    /// Create a new config with the given LED count and revision.
    #[must_use]
    pub const fn new(led_count: usize, revision: ProtocolRevision) -> Self {
        Self {
            led_count,
            revision,
            leds_per_update: None,
            matrix: None,
            has_underglow: false,
            underglow_count: 0,
        }
    }

    /// Set optional matrix dimensions for zone topology.
    #[must_use]
    pub const fn with_matrix(mut self, rows: u32, cols: u32) -> Self {
        self.matrix = Some((rows, cols));
        self
    }

    /// Override the batch size for slower MCUs.
    #[must_use]
    pub const fn with_leds_per_update(mut self, count: usize) -> Self {
        self.leds_per_update = Some(count);
        self
    }

    /// Configure underglow zone.
    #[must_use]
    pub const fn with_underglow(mut self, count: usize) -> Self {
        self.has_underglow = true;
        self.underglow_count = count;
        self
    }

    /// Effective batch size for `DIRECT_MODE_SET_LEDS`.
    #[must_use]
    const fn effective_leds_per_update(&self) -> usize {
        match self.leds_per_update {
            Some(override_count) => override_count,
            None => self.revision.max_leds_per_update(),
        }
    }

    /// Key LED count (total minus underglow).
    #[must_use]
    const fn key_led_count(&self) -> usize {
        if self.has_underglow {
            self.led_count.saturating_sub(self.underglow_count)
        } else {
            self.led_count
        }
    }
}

// ── Protocol implementation ──────────────────────────────────────────────

/// QMK HID RGB protocol encoder/decoder.
#[derive(Debug, Clone)]
pub struct QmkProtocol {
    config: QmkKeyboardConfig,
}

impl QmkProtocol {
    /// Create a new QMK protocol instance.
    #[must_use]
    pub const fn new(config: QmkKeyboardConfig) -> Self {
        Self { config }
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

        if colors.len() != expected {
            warn!(
                expected,
                actual = colors.len(),
                "qmk frame length mismatch; applying truncate/pad"
            );
        }

        Cow::Owned(normalized)
    }

    /// Encode a batch of LED colors into `DIRECT_MODE_SET_LEDS` packets.
    fn encode_direct_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let normalized = self.normalize_colors(colors);
        let normalized = normalized.as_ref();
        let batch_size = self.config.effective_leds_per_update();
        let mut command_buffer = CommandBuffer::new(commands);
        let mut led_offset: usize = 0;

        for chunk in normalized.chunks(batch_size) {
            let mut packet = QmkPacket::new_zeroed();
            packet.command = Command::DirectModeSetLeds.byte();

            match self.config.revision {
                ProtocolRevision::Rev9 | ProtocolRevision::RevB => {
                    // RevB format: [start_idx, count, R, G, B, R, G, B, ...]
                    packet.payload[0] = u8::try_from(led_offset).unwrap_or(u8::MAX);
                    packet.payload[1] = u8::try_from(chunk.len()).unwrap_or(u8::MAX);
                    for (led_idx, color) in chunk.iter().enumerate() {
                        let offset = 2 + led_idx * 3;
                        if offset + 3 <= packet.payload.len() {
                            packet.payload[offset] = color[0];
                            packet.payload[offset + 1] = color[1];
                            packet.payload[offset + 2] = color[2];
                        }
                    }
                }
                ProtocolRevision::RevD => {
                    // RevD format: [count, led_value, R, G, B, led_value, R, G, B, ...]
                    packet.payload[0] = u8::try_from(chunk.len()).unwrap_or(u8::MAX);
                    for (led_idx, color) in chunk.iter().enumerate() {
                        let offset = 1 + led_idx * 4;
                        if offset + 4 <= packet.payload.len() {
                            let global_idx = led_offset + led_idx;
                            packet.payload[offset] = u8::try_from(global_idx).unwrap_or(u8::MAX);
                            packet.payload[offset + 1] = color[0];
                            packet.payload[offset + 2] = color[1];
                            packet.payload[offset + 3] = color[2];
                        }
                    }
                }
            }

            command_buffer.push_struct(
                &packet,
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
            );

            led_offset += chunk.len();
        }

        command_buffer.finish();
    }
}

impl Protocol for QmkProtocol {
    fn name(&self) -> &'static str {
        "QMK HID RGB"
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        let mut commands = Vec::with_capacity(2);

        // Query protocol version to verify device compatibility.
        let mut version_query = QmkPacket::new_zeroed();
        version_query.command = Command::GetProtocolVersion.byte();
        commands.push(command_from_packet(&version_query, true));

        // Switch to Direct mode (mode 1) with full brightness, no EEPROM save.
        let mut set_mode = QmkPacket::new_zeroed();
        set_mode.command = Command::SetMode.byte();
        set_mode.payload[0] = 0x00; // hue
        set_mode.payload[1] = 0x00; // saturation
        set_mode.payload[2] = 0xFF; // value (brightness)
        set_mode.payload[3] = 0x01; // mode = Direct
        set_mode.payload[4] = SPEED_NORMAL;
        set_mode.payload[5] = 0x00; // save = false
        commands.push(command_from_packet(&set_mode, true));

        commands
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        // Restore Solid Color mode (mode 2) so the keyboard reverts to its
        // built-in effect. No EEPROM save — just a session-level restore.
        let mut set_mode = QmkPacket::new_zeroed();
        set_mode.command = Command::SetMode.byte();
        set_mode.payload[0] = 0x00; // hue
        set_mode.payload[1] = 0xFF; // saturation
        set_mode.payload[2] = 0xFF; // value
        set_mode.payload[3] = 0x02; // mode = SOLID_COLOR
        set_mode.payload[4] = SPEED_NORMAL;
        set_mode.payload[5] = 0x00; // save = false

        vec![command_from_packet(&set_mode, true)]
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        self.encode_direct_frame_into(colors, commands);
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        if data.is_empty() {
            return Err(ProtocolError::MalformedResponse {
                detail: "empty QMK response".to_string(),
            });
        }

        // Check for failure sentinel in LED info responses.
        if data.len() >= 4 && data[3] == STATUS_FAILURE {
            return Err(ProtocolError::DeviceError {
                status: ResponseStatus::Failed,
            });
        }

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data.to_vec(),
        })
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        let mut zones = Vec::with_capacity(2);

        let key_count = self.config.key_led_count();
        let topology = match self.config.matrix {
            Some((rows, cols)) => DeviceTopologyHint::Matrix { rows, cols },
            None => DeviceTopologyHint::Strip,
        };

        zones.push(ProtocolZone {
            name: "Keyboard".to_owned(),
            led_count: u32::try_from(key_count).unwrap_or(u32::MAX),
            topology,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        });

        if self.config.has_underglow && self.config.underglow_count > 0 {
            zones.push(ProtocolZone {
                name: "Underglow".to_owned(),
                led_count: u32::try_from(self.config.underglow_count).unwrap_or(u32::MAX),
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            });
        }

        zones
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: 30,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        u32::try_from(self.config.led_count).unwrap_or(u32::MAX)
    }

    fn frame_interval(&self) -> Duration {
        // QMK keyboards are typically USB full-speed with serial HID I/O,
        // so we target ~30 FPS to avoid overwhelming the MCU.
        Duration::from_millis(33)
    }
}

fn command_from_packet(packet: &QmkPacket, expects_response: bool) -> ProtocolCommand {
    ProtocolCommand {
        data: packet.as_bytes().to_vec(),
        expects_response,
        response_delay: Duration::from_millis(super::types::HID_READ_TIMEOUT_MS),
        post_delay: Duration::ZERO,
        transfer_type: TransferType::Primary,
    }
}
