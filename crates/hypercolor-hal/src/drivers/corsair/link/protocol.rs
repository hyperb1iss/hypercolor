//! Native Corsair iCUE LINK hub protocol.

use std::sync::RwLock;
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceTopologyHint,
};
use tracing::warn;

use crate::drivers::corsair::CORSAIR_KEEPALIVE_INTERVAL;
use crate::drivers::corsair::framing::{
    LINK_MAX_PAYLOAD, build_link_packet, build_link_write_buffer, chunk_bytes,
};
use crate::drivers::corsair::types::{
    EP_GET_DEVICES, EP_SET_COLOR, EndpointConfig, LinkCommand, LinkDeviceType,
};
use crate::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolKeepalive, ProtocolResponse, ProtocolZone,
    ResponseStatus, TransferType,
};

const DEFAULT_TARGET_FPS: u32 = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkChild {
    /// Enumerated downstream type.
    pub device_type: LinkDeviceType,
    /// Vendor-specific model byte.
    pub model: u8,
    /// Raw serial/endpoint identifier string.
    pub serial: String,
    /// LED count contributed by this child.
    pub led_count: u32,
    /// Flat RGB offset within the hub frame.
    pub color_offset: u32,
}

impl LinkChild {
    /// Human-readable zone name for this child.
    #[must_use]
    pub fn zone_name(&self) -> String {
        if self.serial.is_empty() {
            self.device_type.display_name().to_owned()
        } else {
            format!("{} ({})", self.device_type.display_name(), self.serial)
        }
    }

    /// Best-effort topology hint.
    #[must_use]
    pub const fn topology(&self) -> DeviceTopologyHint {
        self.device_type.topology_hint(self.model)
    }
}

#[derive(Debug, Clone, Default)]
struct LinkState {
    children: Vec<LinkChild>,
    total_leds: u32,
    last_frame_commands: Vec<ProtocolCommand>,
}

/// Native iCUE LINK hub-and-spoke protocol.
pub struct CorsairLinkProtocol {
    state: RwLock<LinkState>,
}

impl CorsairLinkProtocol {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: RwLock::new(LinkState::default()),
        }
    }

    #[must_use]
    pub fn children(&self) -> Vec<LinkChild> {
        self.state
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .children
            .clone()
    }

    fn command(command: LinkCommand, data: &[u8], expects_response: bool) -> ProtocolCommand {
        ProtocolCommand {
            data: build_link_packet(command.bytes(), data),
            expects_response,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        }
    }

    fn endpoint_command(
        command: LinkCommand,
        endpoint: EndpointConfig,
        expects_response: bool,
    ) -> ProtocolCommand {
        Self::command(command, &[endpoint.address], expects_response)
    }

    fn current_total_leds(&self) -> usize {
        usize::try_from(
            self.state
                .read()
                .unwrap_or_else(|err| err.into_inner())
                .total_leds,
        )
        .unwrap_or_default()
    }

    fn normalize_colors(&self, colors: &[[u8; 3]]) -> Result<Vec<[u8; 3]>, ProtocolError> {
        let expected = self.current_total_leds();
        if expected == 0 {
            return Ok(Vec::new());
        }

        if colors.len() != expected {
            return Err(ProtocolError::EncodingError {
                detail: format!(
                    "corsair LINK frame length mismatch: expected {expected} LEDs, got {}",
                    colors.len()
                ),
            });
        }

        Ok(colors.to_vec())
    }

    fn parse_children_response(data: &[u8]) -> Result<Vec<LinkChild>, ProtocolError> {
        let Some(&channel_count) = data.get(6) else {
            return Err(ProtocolError::MalformedResponse {
                detail: "LINK device enumeration missing channel count".to_owned(),
            });
        };

        let index = data
            .get(7..)
            .ok_or_else(|| ProtocolError::MalformedResponse {
                detail: "LINK device enumeration missing record payload".to_owned(),
            })?;

        let mut children = Vec::new();
        let mut pos = 0_usize;
        let mut color_offset = 0_u32;

        for _ in 0..usize::from(channel_count) {
            let metadata =
                index
                    .get(pos..pos + 8)
                    .ok_or_else(|| ProtocolError::MalformedResponse {
                        detail: format!("LINK child record truncated at offset {pos}"),
                    })?;

            let serial_len = usize::from(metadata[7]);
            if serial_len == 0 {
                pos += 8;
                continue;
            }

            let serial_bytes = index.get(pos + 8..pos + 8 + serial_len).ok_or_else(|| {
                ProtocolError::MalformedResponse {
                    detail: format!("LINK child serial truncated at offset {pos}"),
                }
            })?;

            let Some(device_type) = LinkDeviceType::from_byte(metadata[2]) else {
                pos += 8 + serial_len;
                continue;
            };

            let model = metadata[3];
            let led_count = device_type.led_count(model);
            if device_type.is_internal() || led_count == 0 {
                pos += 8 + serial_len;
                continue;
            }

            let serial = String::from_utf8(serial_bytes.to_vec()).map_err(|error| {
                ProtocolError::MalformedResponse {
                    detail: format!("invalid LINK child serial: {error}"),
                }
            })?;

            children.push(LinkChild {
                device_type,
                model,
                serial,
                led_count,
                color_offset,
            });
            color_offset = color_offset.saturating_add(led_count);
            pos += 8 + serial_len;
        }

        Ok(children)
    }

    fn update_children(&self, children: Vec<LinkChild>) {
        let total_leds = children.iter().map(|child| child.led_count).sum();
        let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
        state.children = children;
        state.total_leds = total_leds;
    }
}

impl Default for CorsairLinkProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl Protocol for CorsairLinkProtocol {
    fn name(&self) -> &'static str {
        "Corsair iCUE LINK System Hub"
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        vec![
            Self::command(LinkCommand::GetFirmware, &[], true),
            Self::command(LinkCommand::SoftwareMode, &[], true),
            Self::endpoint_command(LinkCommand::CloseEndpoint, EP_GET_DEVICES, true),
            Self::endpoint_command(LinkCommand::OpenEndpoint, EP_GET_DEVICES, true),
            Self::command(LinkCommand::Read, &[], true),
            Self::endpoint_command(LinkCommand::CloseEndpoint, EP_GET_DEVICES, true),
        ]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        vec![Self::command(LinkCommand::HardwareMode, &[], true)]
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let normalized = match self.normalize_colors(colors) {
            Ok(frame) => frame,
            Err(error) => {
                warn!(%error, "corsair LINK encode_frame rejected frame");
                return Vec::new();
            }
        };
        if normalized.is_empty() {
            return Vec::new();
        }

        let rgb = normalized
            .iter()
            .flat_map(|color| [color[0], color[1], color[2]])
            .collect::<Vec<_>>();
        let framed = build_link_write_buffer(EP_SET_COLOR.data_type, &rgb);
        let chunks = chunk_bytes(&framed, LINK_MAX_PAYLOAD);

        let mut commands = Vec::with_capacity(chunks.len().saturating_add(3));
        commands.push(Self::endpoint_command(
            LinkCommand::CloseEndpoint,
            EP_SET_COLOR,
            true,
        ));
        commands.push(Self::endpoint_command(
            LinkCommand::OpenColorEndpoint,
            EP_SET_COLOR,
            true,
        ));

        for (index, chunk) in chunks.iter().enumerate() {
            let command = if index == 0 {
                LinkCommand::WriteColor
            } else {
                LinkCommand::WriteColorNext
            };
            commands.push(Self::command(command, chunk, true));
        }

        commands.push(Self::endpoint_command(
            LinkCommand::CloseEndpoint,
            EP_SET_COLOR,
            true,
        ));

        self.state
            .write()
            .unwrap_or_else(|err| err.into_inner())
            .last_frame_commands
            .clone_from(&commands);

        commands
    }

    fn keepalive(&self) -> Option<ProtocolKeepalive> {
        Some(ProtocolKeepalive {
            commands: Vec::new(),
            interval: CORSAIR_KEEPALIVE_INTERVAL,
        })
    }

    fn keepalive_commands(&self) -> Vec<ProtocolCommand> {
        self.state
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .last_frame_commands
            .clone()
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        if data.len() >= 6 && data[4..6] == EP_GET_DEVICES.data_type {
            let children = Self::parse_children_response(data)?;
            self.update_children(children);

            return Ok(ProtocolResponse {
                status: ResponseStatus::Ok,
                data: data[6..].to_vec(),
            });
        }

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data.to_vec(),
        })
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        self.state
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .children
            .iter()
            .map(|child| ProtocolZone {
                name: child.zone_name(),
                led_count: child.led_count,
                topology: child.topology(),
                color_format: DeviceColorFormat::Rgb,
            })
            .collect()
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let total_leds = self
            .state
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .total_leds;
        DeviceCapabilities {
            led_count: total_leds,
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: DEFAULT_TARGET_FPS,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        self.state
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .total_leds
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(33)
    }
}
