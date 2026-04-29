//! ASUS Aura USB motherboard/addressable/terminal protocol.

use std::borrow::Cow;
use std::sync::RwLock;
use std::time::Duration;

#[cfg(target_os = "linux")]
use std::fs;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceTopologyHint,
};
use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone,
    ResponseStatus, TransferType,
};

use super::types::{
    AURA_DIRECT_LED_CHUNK, AURA_DIRECT_LED_MAX, AURA_REPORT_ID, AURA_REPORT_PAYLOAD_LEN,
    AURA_TERMINAL_CHANNEL_LEDS, AuraColorOrder, AuraCommand, AuraControllerGen, AuraInitPhase,
    MAINBOARD_DIRECT_IDX, led_mask,
};

const FIRMWARE_RESPONSE_MARKER: u8 = 0x02;
const CONFIG_RESPONSE_MARKER: u8 = 0x30;
const DIRECT_MODE_ID: u8 = 0xFF;
const RESPONSE_DELAY: Duration = Duration::from_millis(50);
const RESPONSE_POST_DELAY: Duration = Duration::from_millis(5);
const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

const _: () = assert!(
    std::mem::size_of::<AuraDirectPacket>() == AURA_REPORT_PAYLOAD_LEN,
    "AuraDirectPacket must match AURA_REPORT_PAYLOAD_LEN (64 bytes)"
);

/// Wire-format ASUS Aura direct color packet (64 bytes).
///
/// Each packet writes up to 20 RGB triples for a single channel. The last
/// chunk in a channel sets the apply flag (`0x80`) in the channel byte.
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct AuraDirectPacket {
    /// Command byte — always `0x40` (`DirectControl`).
    command: u8,
    /// Channel index OR'd with the apply flag (`0x80`) on the final chunk.
    channel_with_flag: u8,
    /// First LED offset in this chunk.
    led_offset: u8,
    /// Number of LEDs in this chunk.
    chunk_len: u8,
    /// Interleaved color bytes (up to 20 LEDs × 3 = 60 bytes).
    colors: [u8; 60],
}

#[derive(Debug, Clone, Copy)]
struct BoardTopologyOverride {
    mainboard_leds: u32,
    argb_channels: usize,
    rgb_headers: u32,
}

#[derive(Debug, Clone)]
struct AuraTopology {
    firmware: Option<String>,
    mainboard_leds: u32,
    argb_led_counts: Vec<u32>,
    rgb_header_count: u32,
    overrides_applied: bool,
    init_phase: AuraInitPhase,
}

impl AuraTopology {
    fn placeholder(controller_gen: AuraControllerGen) -> Self {
        let (mainboard_leds, argb_led_counts, rgb_header_count) = match controller_gen {
            AuraControllerGen::Motherboard => (1, vec![1, 1, 1], 1),
            AuraControllerGen::AddressableOnly => (0, vec![1, 1, 1, 1], 0),
            AuraControllerGen::Terminal => (0, vec![90, 90, 90, 90, 1], 0),
        };

        Self {
            firmware: None,
            mainboard_leds,
            argb_led_counts,
            rgb_header_count,
            overrides_applied: matches!(controller_gen, AuraControllerGen::Terminal),
            init_phase: if matches!(controller_gen, AuraControllerGen::Terminal) {
                AuraInitPhase::Configured
            } else {
                AuraInitPhase::PreInit
            },
        }
    }
}

/// ASUS Aura USB protocol encoder/decoder with runtime topology discovery.
pub struct AuraUsbProtocol {
    controller_gen: AuraControllerGen,
    color_order: AuraColorOrder,
    needs_gen1_disable: bool,
    discover_topology: bool,
    board_name: Option<String>,
    argb_led_defaults: Vec<u32>,
    topology: RwLock<AuraTopology>,
}

impl AuraUsbProtocol {
    /// Create a new ASUS Aura protocol for one controller family.
    #[must_use]
    pub fn new(controller_gen: AuraControllerGen) -> Self {
        let discover_topology = controller_gen.uses_runtime_discovery();
        let argb_led_defaults = match controller_gen {
            AuraControllerGen::Motherboard | AuraControllerGen::AddressableOnly => {
                vec![AURA_DIRECT_LED_MAX; 4]
            }
            AuraControllerGen::Terminal => vec![
                AURA_TERMINAL_CHANNEL_LEDS,
                AURA_TERMINAL_CHANNEL_LEDS,
                AURA_TERMINAL_CHANNEL_LEDS,
                AURA_TERMINAL_CHANNEL_LEDS,
                1,
            ],
        };

        Self {
            controller_gen,
            color_order: AuraColorOrder::default(),
            needs_gen1_disable: false,
            discover_topology,
            board_name: read_dmi_board_name(),
            argb_led_defaults,
            topology: RwLock::new(AuraTopology::placeholder(controller_gen)),
        }
    }

    /// Override the wire color order for this protocol instance.
    #[must_use]
    pub const fn with_color_order(mut self, color_order: AuraColorOrder) -> Self {
        self.color_order = color_order;
        self
    }

    /// Override the default per-header LED counts used once channel discovery
    /// knows how many headers are present.
    #[must_use]
    pub fn with_argb_led_counts(mut self, argb_led_counts: Vec<u32>) -> Self {
        self.argb_led_defaults.clone_from(&argb_led_counts);

        if !self.discover_topology {
            let mut topology = self
                .topology
                .write()
                .expect("ASUS topology lock should not be poisoned");
            topology.argb_led_counts = argb_led_counts;
        }

        self
    }

    /// Override the detected DMI board name.
    #[must_use]
    pub fn with_board_name(mut self, board_name: impl Into<String>) -> Self {
        self.board_name = Some(board_name.into());
        self
    }

    /// Enable or disable the older Gen1 compatibility command.
    #[must_use]
    pub const fn with_gen1_disable(mut self, needs_gen1_disable: bool) -> Self {
        self.needs_gen1_disable = needs_gen1_disable;
        self
    }

    /// Seed a fully known topology, primarily for tests and static variants.
    #[must_use]
    pub fn with_topology(mut self, mainboard_leds: u32, argb_led_counts: Vec<u32>) -> Self {
        self.argb_led_defaults.clone_from(&argb_led_counts);
        self.discover_topology = false;

        let mut topology = self
            .topology
            .write()
            .expect("ASUS topology lock should not be poisoned");
        topology.mainboard_leds = mainboard_leds;
        topology.argb_led_counts = argb_led_counts;
        topology.overrides_applied = true;
        topology.init_phase = AuraInitPhase::Configured;
        drop(topology);

        self
    }

    /// Latest parsed firmware string, if any.
    #[must_use]
    pub fn firmware(&self) -> Option<String> {
        self.topology
            .read()
            .expect("ASUS topology lock should not be poisoned")
            .firmware
            .clone()
    }

    fn controller_name(&self) -> &'static str {
        match self.controller_gen {
            AuraControllerGen::Motherboard => "ASUS Aura Motherboard",
            AuraControllerGen::AddressableOnly => "ASUS Aura Addressable",
            AuraControllerGen::Terminal => "ASUS Aura Terminal",
        }
    }

    fn current_total_leds(&self) -> usize {
        usize::try_from(self.total_leds()).unwrap_or_default()
    }

    fn normalize_frame_colors<'a>(&self, colors: &'a [[u8; 3]]) -> Cow<'a, [[u8; 3]]> {
        let expected = self.current_total_leds();
        if expected == 0 {
            return Cow::Borrowed(&[]);
        }
        if colors.len() == expected {
            return Cow::Borrowed(colors);
        }

        let mut normalized = vec![[0_u8; 3]; expected];
        let copy_len = colors.len().min(expected);
        normalized[..copy_len].copy_from_slice(&colors[..copy_len]);
        Cow::Owned(normalized)
    }

    fn resolved_argb_led_counts(&self, channel_count: usize) -> Vec<u32> {
        if channel_count == 0 {
            return Vec::new();
        }

        let mut counts = if self.argb_led_defaults.is_empty() {
            vec![AURA_DIRECT_LED_MAX; channel_count]
        } else {
            self.argb_led_defaults.clone()
        };

        if counts.len() < channel_count {
            counts.resize(channel_count, AURA_DIRECT_LED_MAX);
        }
        counts.truncate(channel_count);
        counts
    }

    fn build_query_command(command: AuraCommand) -> ProtocolCommand {
        ProtocolCommand {
            data: zeroed_payload(command),
            expects_response: true,
            response_delay: RESPONSE_DELAY,
            post_delay: RESPONSE_POST_DELAY,
            transfer_type: TransferType::Primary,
        }
    }

    fn build_write_command(data: Vec<u8>) -> ProtocolCommand {
        ProtocolCommand {
            data,
            expects_response: false,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        }
    }

    fn direct_mode_commands(&self) -> Vec<ProtocolCommand> {
        match self.controller_gen {
            AuraControllerGen::Motherboard => {
                let mut commands = (0_u8..self.preferred_argb_channel_count())
                    .map(build_set_direct_mode_payload)
                    .map(Self::build_write_command)
                    .collect::<Vec<_>>();
                commands.push(Self::build_write_command(build_set_direct_mode_payload(
                    MAINBOARD_DIRECT_IDX,
                )));
                commands
            }
            AuraControllerGen::AddressableOnly => (0_u8..self.preferred_argb_channel_count())
                .map(build_set_direct_mode_payload)
                .map(Self::build_write_command)
                .collect(),
            AuraControllerGen::Terminal => (0_u8..5)
                .map(build_set_addressable_direct_mode_payload)
                .map(Self::build_write_command)
                .collect(),
        }
    }

    fn preferred_argb_channel_count(&self) -> u8 {
        let topology = self
            .topology
            .read()
            .expect("ASUS topology lock should not be poisoned");

        if topology.init_phase == AuraInitPhase::Configured {
            u8::try_from(topology.argb_led_counts.len()).unwrap_or(u8::MAX)
        } else {
            match self.controller_gen {
                AuraControllerGen::Motherboard | AuraControllerGen::AddressableOnly => 4,
                AuraControllerGen::Terminal => 5,
            }
        }
    }

    fn apply_override(&self, topology: &mut AuraTopology, override_entry: BoardTopologyOverride) {
        topology.mainboard_leds = override_entry.mainboard_leds;
        topology.argb_led_counts = self.resolved_argb_led_counts(override_entry.argb_channels);
        topology.rgb_header_count = override_entry.rgb_headers;
        topology.overrides_applied = true;
    }
}

impl Default for AuraUsbProtocol {
    fn default() -> Self {
        Self::new(AuraControllerGen::Motherboard)
    }
}

impl Protocol for AuraUsbProtocol {
    fn name(&self) -> &'static str {
        self.controller_name()
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();

        if self.discover_topology {
            commands.push(Self::build_query_command(AuraCommand::FirmwareVersion));
            commands.push(Self::build_query_command(AuraCommand::ConfigTable));
        }

        if self.needs_gen1_disable {
            commands.push(Self::build_write_command(build_disable_gen2_payload()));
        }

        commands.extend(self.direct_mode_commands());
        commands
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let normalized = self.normalize_frame_colors(colors);
        if normalized.is_empty() {
            commands.truncate(0);
            return;
        }

        let topology = self
            .topology
            .read()
            .expect("ASUS topology lock should not be poisoned")
            .clone();
        let mut command_buffer = CommandBuffer::new(commands);
        let mut cursor = 0_usize;

        if matches!(self.controller_gen, AuraControllerGen::Motherboard)
            && topology.mainboard_leds > 0
        {
            let count = usize::try_from(topology.mainboard_leds).unwrap_or_default();
            let end = cursor.saturating_add(count).min(normalized.len());
            encode_channel_direct_packets_into(
                &mut command_buffer,
                MAINBOARD_DIRECT_IDX,
                &normalized[cursor..end],
                self.color_order,
            );
            cursor = end;
        }

        for (channel_index, led_count) in topology.argb_led_counts.iter().enumerate() {
            let count = usize::try_from(*led_count).unwrap_or_default();
            if count == 0 || cursor >= normalized.len() {
                continue;
            }

            let end = cursor.saturating_add(count).min(normalized.len());
            encode_channel_direct_packets_into(
                &mut command_buffer,
                u8::try_from(channel_index).unwrap_or(u8::MAX),
                &normalized[cursor..end],
                self.color_order,
            );
            cursor = end;
        }

        command_buffer.finish();
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        let payload = strip_report_id(data);
        let Some(&marker) = payload.first() else {
            return Err(ProtocolError::MalformedResponse {
                detail: "ASUS response is empty".to_owned(),
            });
        };

        match marker {
            FIRMWARE_RESPONSE_MARKER => {
                let firmware = parse_firmware_response(payload)?;
                let mut topology = self
                    .topology
                    .write()
                    .expect("ASUS topology lock should not be poisoned");
                let board_name = self
                    .board_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                let firmware_override = lookup_firmware_override(&firmware);
                let board_override = board_name.and_then(lookup_board_name_override);
                topology.firmware = Some(firmware.clone());
                topology.init_phase = AuraInitPhase::FirmwareReceived;

                if let Some(override_entry) = firmware_override.or(board_override) {
                    self.apply_override(&mut topology, override_entry);
                }

                Ok(ProtocolResponse {
                    status: ResponseStatus::Ok,
                    data: firmware.into_bytes(),
                })
            }
            CONFIG_RESPONSE_MARKER => {
                let table = parse_config_table(payload)?;
                let discovered_argb_channels = usize::from(table[0x02]);
                let discovered_mainboard_leds = u32::from(table[0x1B]);
                let discovered_rgb_headers = u32::from(table[0x1D]);
                let mut topology = self
                    .topology
                    .write()
                    .expect("ASUS topology lock should not be poisoned");

                if !topology.overrides_applied {
                    topology.argb_led_counts =
                        self.resolved_argb_led_counts(discovered_argb_channels);
                    topology.mainboard_leds = discovered_mainboard_leds;
                    topology.rgb_header_count = discovered_rgb_headers;
                }

                topology.init_phase = AuraInitPhase::Configured;

                Ok(ProtocolResponse {
                    status: ResponseStatus::Ok,
                    data: table.to_vec(),
                })
            }
            _ => Ok(ProtocolResponse {
                status: ResponseStatus::Unsupported,
                data: payload.to_vec(),
            }),
        }
    }

    fn response_timeout(&self) -> Duration {
        RESPONSE_TIMEOUT
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        let topology = self
            .topology
            .read()
            .expect("ASUS topology lock should not be poisoned");
        let mut zones = Vec::new();

        if matches!(self.controller_gen, AuraControllerGen::Motherboard)
            && topology.mainboard_leds > 0
        {
            zones.push(ProtocolZone {
                name: "Mainboard".to_owned(),
                led_count: topology.mainboard_leds,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            });
        }

        for (index, led_count) in topology.argb_led_counts.iter().enumerate() {
            let (name, topology_hint) =
                if matches!(self.controller_gen, AuraControllerGen::Terminal) && index == 4 {
                    ("Logo".to_owned(), DeviceTopologyHint::Point)
                } else if matches!(self.controller_gen, AuraControllerGen::Terminal) {
                    (
                        format!("ARGB Channel {}", index + 1),
                        DeviceTopologyHint::Strip,
                    )
                } else {
                    (
                        format!("ARGB Header {}", index + 1),
                        DeviceTopologyHint::Strip,
                    )
                };

            zones.push(ProtocolZone {
                name,
                led_count: *led_count,
                topology: topology_hint,
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
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        let topology = self
            .topology
            .read()
            .expect("ASUS topology lock should not be poisoned");
        topology.mainboard_leds + topology.argb_led_counts.iter().sum::<u32>()
    }

    fn frame_interval(&self) -> Duration {
        FRAME_INTERVAL
    }
}

/// Build the effect-color payload used by the hardware-effect command.
///
/// The returned bytes exclude the HID report ID; the transport prepends
/// [`AURA_REPORT_ID`] on write.
///
/// # Errors
///
/// Returns [`ProtocolError`] when the LED window exceeds the 16-bit mask or
/// the payload would overrun the fixed report size.
pub fn build_effect_color_payload(
    start_led: u8,
    colors: &[[u8; 3]],
    shutdown: bool,
    color_order: AuraColorOrder,
) -> Result<Vec<u8>, ProtocolError> {
    let count = u8::try_from(colors.len()).map_err(|_| ProtocolError::EncodingError {
        detail: "ASUS effect color packet supports at most 255 LEDs".to_owned(),
    })?;
    let mask = led_mask(start_led, count);

    if count > 0 && mask == 0 {
        return Err(ProtocolError::EncodingError {
            detail: "ASUS effect color LED mask overflow".to_owned(),
        });
    }

    let mut payload = zeroed_payload(AuraCommand::SetEffectColor);
    payload[1] = u8::try_from(mask >> 8).unwrap_or_default();
    payload[2] = u8::try_from(mask & 0x00FF).unwrap_or_default();
    payload[3] = u8::from(shutdown);

    for (index, [r, g, b]) in colors.iter().copied().enumerate() {
        let start = 4 + (usize::from(start_led) + index) * 3;
        let end = start + 3;
        if end > payload.len() {
            return Err(ProtocolError::EncodingError {
                detail: "ASUS effect color packet exceeds report size".to_owned(),
            });
        }

        payload[start..end].copy_from_slice(&color_order.permute(r, g, b));
    }

    Ok(payload)
}

fn strip_report_id(data: &[u8]) -> &[u8] {
    if data.first().copied() == Some(AURA_REPORT_ID) {
        &data[1..]
    } else {
        data
    }
}

fn zeroed_payload(command: AuraCommand) -> Vec<u8> {
    let mut payload = vec![0_u8; AURA_REPORT_PAYLOAD_LEN];
    payload[0] = u8::from(command);
    payload
}

fn build_disable_gen2_payload() -> Vec<u8> {
    let mut payload = zeroed_payload(AuraCommand::DisableGen2);
    payload[1] = 0x53;
    payload[3] = 0x01;
    payload
}

fn build_set_direct_mode_payload(channel_index: u8) -> Vec<u8> {
    let mut payload = zeroed_payload(AuraCommand::SetMode);
    payload[1] = channel_index;
    payload[4] = DIRECT_MODE_ID;
    payload
}

fn build_set_addressable_direct_mode_payload(channel_index: u8) -> Vec<u8> {
    let mut payload = zeroed_payload(AuraCommand::SetAddressableMode);
    payload[1] = channel_index;
    payload[3] = DIRECT_MODE_ID;
    payload
}

fn encode_channel_direct_packets_into(
    command_buffer: &mut CommandBuffer<'_>,
    channel_index: u8,
    colors: &[[u8; 3]],
    color_order: AuraColorOrder,
) {
    if colors.is_empty() {
        return;
    }

    let chunk_count = colors.len().div_ceil(AURA_DIRECT_LED_CHUNK);
    for (chunk_index, chunk) in colors.chunks(AURA_DIRECT_LED_CHUNK).enumerate() {
        let mut packet = AuraDirectPacket::new_zeroed();
        packet.command = u8::from(AuraCommand::DirectControl);
        packet.channel_with_flag = channel_index
            | if chunk_index + 1 == chunk_count {
                0x80
            } else {
                0x00
            };
        packet.led_offset = u8::try_from(chunk_index * AURA_DIRECT_LED_CHUNK)
            .expect("ASUS direct packet offsets fit into one byte");
        packet.chunk_len =
            u8::try_from(chunk.len()).expect("ASUS direct packet chunk length fits into u8");

        for (index, [r, g, b]) in chunk.iter().copied().enumerate() {
            let [wire_r, wire_g, wire_b] = color_order.permute(r, g, b);
            let offset = index * 3;
            packet.colors[offset] = wire_r;
            packet.colors[offset + 1] = wire_g;
            packet.colors[offset + 2] = wire_b;
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

fn parse_firmware_response(payload: &[u8]) -> Result<String, ProtocolError> {
    let bytes = payload
        .get(1..17)
        .ok_or_else(|| ProtocolError::MalformedResponse {
            detail: "ASUS firmware response missing 16-byte version string".to_owned(),
        })?;
    let raw =
        String::from_utf8(bytes.to_vec()).map_err(|error| ProtocolError::MalformedResponse {
            detail: format!("ASUS firmware response is not valid ASCII: {error}"),
        })?;

    Ok(raw.trim_end_matches('\0').trim().to_owned())
}

fn parse_config_table(payload: &[u8]) -> Result<&[u8], ProtocolError> {
    payload
        .get(3..63)
        .ok_or_else(|| ProtocolError::MalformedResponse {
            detail: "ASUS config response missing 60-byte configuration table".to_owned(),
        })
}

fn read_dmi_board_name() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let raw = fs::read_to_string("/sys/class/dmi/id/board_name").ok()?;
        let board_name = raw.trim();
        (!board_name.is_empty()).then(|| board_name.to_owned())
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn lookup_firmware_override(firmware: &str) -> Option<BoardTopologyOverride> {
    match firmware {
        "AULA3-AR32-0207" => Some(BoardTopologyOverride {
            mainboard_leds: 3,
            argb_channels: 3,
            rgb_headers: 1,
        }),
        "AULA3-AR32-0213" => Some(BoardTopologyOverride {
            mainboard_leds: 2,
            argb_channels: 3,
            rgb_headers: 1,
        }),
        "AULA3-AR32-0218" => Some(BoardTopologyOverride {
            mainboard_leds: 5,
            argb_channels: 3,
            rgb_headers: 1,
        }),
        _ => None,
    }
}

fn lookup_board_name_override(board_name: &str) -> Option<BoardTopologyOverride> {
    match board_name {
        "ROG CROSSHAIR VIII HERO" => Some(BoardTopologyOverride {
            mainboard_leds: 8,
            argb_channels: 2,
            rgb_headers: 2,
        }),
        "ROG MAXIMUS Z690 EXTREME" => Some(BoardTopologyOverride {
            mainboard_leds: 7,
            argb_channels: 3,
            rgb_headers: 1,
        }),
        "ROG MAXIMUS Z690 EXTREME GLACIAL" => Some(BoardTopologyOverride {
            mainboard_leds: 7,
            argb_channels: 4,
            rgb_headers: 1,
        }),
        "TUF GAMING X570-PRO (WI-FI)" => Some(BoardTopologyOverride {
            mainboard_leds: 3,
            argb_channels: 1,
            rgb_headers: 2,
        }),
        "PRIME Z790-A WIFI" => Some(BoardTopologyOverride {
            mainboard_leds: 4,
            argb_channels: 3,
            rgb_headers: 1,
        }),
        "ROG STRIX B650-A GAMING WIFI"
        | "ROG STRIX B650E-F GAMING WIFI"
        | "ROG STRIX Z890-E GAMING WIFI" => Some(BoardTopologyOverride {
            mainboard_leds: 3,
            argb_channels: 3,
            rgb_headers: 1,
        }),
        "ROG MAXIMUS Z790 APEX ENCORE" | "ROG STRIX B760-F GAMING WIFI" => {
            Some(BoardTopologyOverride {
                mainboard_leds: 2,
                argb_channels: 3,
                rgb_headers: 1,
            })
        }
        "TUF GAMING Z890-PLUS WIFI" => Some(BoardTopologyOverride {
            mainboard_leds: 1,
            argb_channels: 3,
            rgb_headers: 0,
        }),
        "ROG STRIX Z890-A GAMING WIFI" => Some(BoardTopologyOverride {
            mainboard_leds: 2,
            argb_channels: 3,
            rgb_headers: 0,
        }),
        "PRIME Z890-P WIFI" => Some(BoardTopologyOverride {
            mainboard_leds: 0,
            argb_channels: 3,
            rgb_headers: 1,
        }),
        "ROG STRIX B850-F GAMING WIFI" => Some(BoardTopologyOverride {
            mainboard_leds: 2,
            argb_channels: 3,
            rgb_headers: 2,
        }),
        _ => None,
    }
}
