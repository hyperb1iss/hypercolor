//! Corsair legacy vendor-control lighting commands.

use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceFeatures};

use crate::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone, ResponseStatus,
    TransferType,
};
use crate::transport::vendor::{
    VendorControlOperation, encode_operations as encode_vendor_operations,
};

const LEGACY_TIMEOUT: Duration = Duration::from_secs(5);
const NK95_HWON: u32 = 0x0002_0001;
const NK95_BRIGHTNESS_0: u32 = 0x0031_0000;
const NK95_BRIGHTNESS_33: u32 = 0x0031_0001;
const NK95_BRIGHTNESS_66: u32 = 0x0031_0002;
const NK95_BRIGHTNESS_100: u32 = 0x0031_0003;
const M95_BACKLIGHT_REQUEST: u8 = 49;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyPeripheralKind {
    Keyboard,
    M95Mouse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyPeripheralConfig {
    pub name: &'static str,
    pub kind: LegacyPeripheralKind,
}

#[derive(Debug, Clone)]
pub struct CorsairLegacyPeripheralProtocol {
    config: LegacyPeripheralConfig,
}

impl CorsairLegacyPeripheralProtocol {
    #[must_use]
    pub const fn new(config: LegacyPeripheralConfig) -> Self {
        Self { config }
    }

    fn command(operations: &[VendorControlOperation], expects_response: bool) -> ProtocolCommand {
        let data = encode_vendor_operations(operations)
            .expect("legacy Corsair vendor operation should fit transport framing");
        ProtocolCommand {
            data,
            expects_response,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        }
    }

    fn keyboard_command(command: u32) -> ProtocolCommand {
        let request = u8::try_from((command >> 16) & 0xFF).unwrap_or_default();
        let value = u16::try_from(command & 0xFFFF).unwrap_or_default();
        Self::command(
            &[VendorControlOperation::Write {
                request,
                value,
                index: 0,
                data: Vec::new(),
            }],
            false,
        )
    }

    fn m95_backlight(enabled: bool) -> ProtocolCommand {
        Self::command(
            &[VendorControlOperation::Write {
                request: M95_BACKLIGHT_REQUEST,
                value: u16::from(enabled),
                index: 0,
                data: Vec::new(),
            }],
            false,
        )
    }

    fn brightness_command(brightness: u8) -> ProtocolCommand {
        let command = match brightness {
            0 => NK95_BRIGHTNESS_0,
            1..=85 => NK95_BRIGHTNESS_33,
            86..=170 => NK95_BRIGHTNESS_66,
            _ => NK95_BRIGHTNESS_100,
        };
        Self::keyboard_command(command)
    }
}

impl Protocol for CorsairLegacyPeripheralProtocol {
    fn name(&self) -> &'static str {
        self.config.name
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        match self.config.kind {
            LegacyPeripheralKind::Keyboard => vec![Self::keyboard_command(NK95_HWON)],
            LegacyPeripheralKind::M95Mouse => Vec::new(),
        }
    }

    fn encode_frame(&self, _colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame_into(&self, _colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        commands.clear();
    }

    fn encode_brightness(&self, brightness: u8) -> Option<Vec<ProtocolCommand>> {
        match self.config.kind {
            LegacyPeripheralKind::Keyboard => Some(vec![Self::brightness_command(brightness)]),
            LegacyPeripheralKind::M95Mouse => Some(vec![Self::m95_backlight(brightness > 0)]),
        }
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data.to_vec(),
        })
    }

    fn response_timeout(&self) -> Duration {
        LEGACY_TIMEOUT
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        Vec::new()
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: 0,
            supports_direct: false,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 0,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        0
    }

    fn frame_interval(&self) -> Duration {
        Duration::ZERO
    }
}
