//! Razer Kraken memory-access protocol.
//!
//! Older Kraken headsets do not speak the 90-byte Razer feature report. Their
//! lighting lives in a small RAM/EEPROM map that the host reads and writes
//! through a 37-byte HID output report (`0x04`), with replies arriving as
//! input report `0x05`. Every lighting operation is "write N bytes at address
//! A": a color triplet into the custom-color registers, then an effect
//! bitfield into the LED mode register.
//!
//! See `docs/specs/83-razer-kraken-protocol-driver.md` for the wire format.

use std::borrow::Cow;
use std::cmp::min;
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceFeatures, DeviceTopologyHint,
    SegmentInfo,
};
use tracing::warn;
use zerocopy::byteorder::{BigEndian, U16};
use zerocopy::{FromBytes, FromZeros, Immutable, IntoBytes, KnownLayout};

use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ResponseStatus,
    TransferType,
};

/// HID output report ID for Kraken memory-access requests.
pub const KRAKEN_OUTPUT_REPORT_ID: u8 = 0x04;

/// HID input report ID for Kraken memory-read results.
pub const KRAKEN_INPUT_REPORT_ID: u8 = 0x05;

/// Request body length, excluding the report ID byte the transport prepends.
pub const KRAKEN_REQUEST_BODY_LEN: usize = 36;

/// Full output report length on the wire, including the report ID byte.
pub const KRAKEN_REPORT_LEN: usize = KRAKEN_REQUEST_BODY_LEN + 1;

/// Input report length on the wire, including the report ID byte.
pub const KRAKEN_RESPONSE_LEN: usize = 33;

/// Destination selector: write the payload into RAM.
const DESTINATION_RAM_WRITE: u8 = 0x40;

/// Destination selector: read from EEPROM.
const DESTINATION_EEPROM_READ: u8 = 0x20;

/// RAM address of the custom-color red register (green and blue follow).
const CUSTOM_COLOR_ADDRESS: u16 = 0x1189;

/// RAM address of the LED effect bitfield on the Kraken V2 ("Kylie") map.
const KYLIE_LED_MODE_ADDRESS: u16 = 0x172D;

/// EEPROM address of the two-byte BCD firmware version.
const FIRMWARE_VERSION_ADDRESS: u16 = 0x0030;

/// Firmware version length in bytes (major, minor).
const FIRMWARE_VERSION_LEN: u8 = 2;

/// Effect bitfield value for "LED on, static": bit 0 set, every animation
/// bit clear. The firmware then shows the custom-color registers as-is.
const EFFECT_ON_STATIC: u8 = 0x01;

/// Request argument capacity.
const REQUEST_ARGS_LEN: usize = 32;

/// Response argument capacity.
const RESPONSE_ARGS_LEN: usize = KRAKEN_RESPONSE_LEN - 1;

/// Budget for the firmware-version probe reply.
const DIAGNOSTIC_RESPONSE_TIMEOUT: Duration = Duration::from_millis(250);

/// Device-side frame cap. Each frame is two short RAM writes, so the
/// render loop, not the headset, sets the practical cadence.
const FRAME_INTERVAL: Duration = Duration::from_millis(2);

const _: () = assert!(
    std::mem::size_of::<KrakenRequest>() == KRAKEN_REQUEST_BODY_LEN,
    "KrakenRequest must match the 36-byte output report body"
);

const _: () = assert!(
    std::mem::size_of::<KrakenResponse>() == KRAKEN_RESPONSE_LEN,
    "KrakenResponse must match the 33-byte input report"
);

/// Wire-format Kraken memory-access request (report body, 36 bytes).
///
/// The HID transport prepends report ID `0x04`, so the full output report
/// on the wire is 37 bytes.
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct KrakenRequest {
    /// Memory target and direction (`0x40` RAM write, `0x20` EEPROM read).
    destination: u8,
    /// Number of bytes to read or write at `address`.
    length: u8,
    /// Big-endian memory address.
    address: U16<BigEndian>,
    /// Write payload, zero-padded past `length`.
    arguments: [u8; REQUEST_ARGS_LEN],
}

impl KrakenRequest {
    fn ram_write(address: u16, payload: &[u8]) -> Self {
        let mut request = Self::new_zeroed();
        let length = min(payload.len(), REQUEST_ARGS_LEN);
        request.destination = DESTINATION_RAM_WRITE;
        request.length = u8::try_from(length).expect("request payload length fits in u8");
        request.address = U16::new(address);
        request.arguments[..length].copy_from_slice(&payload[..length]);
        request
    }

    fn eeprom_read(address: u16, length: u8) -> Self {
        let mut request = Self::new_zeroed();
        request.destination = DESTINATION_EEPROM_READ;
        request.length = length;
        request.address = U16::new(address);
        request
    }
}

/// Wire-format Kraken memory-read result (input report, 33 bytes).
#[derive(FromBytes, KnownLayout, Immutable)]
#[repr(C)]
struct KrakenResponse {
    /// Input report ID (`0x05`).
    report_id: u8,
    /// Bytes read from the requested address, in order.
    arguments: [u8; RESPONSE_ARGS_LEN],
}

/// Kraken models driven by this protocol.
///
/// Every model shares the request format; they differ in where the LED mode
/// register lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KrakenModel {
    /// Razer Kraken Ultimate (`1532:0527`), Kraken V2 address map.
    Ultimate,
}

impl KrakenModel {
    const fn led_mode_address(self) -> u16 {
        match self {
            Self::Ultimate => KYLIE_LED_MODE_ADDRESS,
        }
    }
}

/// Protocol encoder for Kraken headsets with memory-mapped lighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KrakenProtocol {
    model: KrakenModel,
}

impl KrakenProtocol {
    /// Create a protocol encoder for one Kraken model.
    #[must_use]
    pub const fn new(model: KrakenModel) -> Self {
        Self { model }
    }

    /// The model this encoder drives.
    #[must_use]
    pub const fn model(&self) -> KrakenModel {
        self.model
    }

    fn effect_request(&self) -> KrakenRequest {
        KrakenRequest::ram_write(self.model.led_mode_address(), &[EFFECT_ON_STATIC])
    }

    fn normalize_colors<'a>(&self, colors: &'a [[u8; 3]]) -> Cow<'a, [[u8; 3]]> {
        let expected = usize::try_from(self.total_leds()).unwrap_or(0);
        if colors.len() == expected {
            return Cow::Borrowed(colors);
        }

        warn!(
            expected,
            actual = colors.len(),
            "razer kraken frame length mismatch; applying truncate/pad"
        );

        let mut normalized = vec![[0_u8; 3]; expected];
        let copy_len = min(colors.len(), expected);
        normalized[..copy_len].copy_from_slice(&colors[..copy_len]);
        Cow::Owned(normalized)
    }

    fn command_for(request: &KrakenRequest) -> ProtocolCommand {
        ProtocolCommand {
            data: request.as_bytes().to_vec(),
            transfer_type: TransferType::Primary,
            ..ProtocolCommand::default()
        }
    }
}

impl Protocol for KrakenProtocol {
    fn name(&self) -> &'static str {
        "Razer Kraken"
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        vec![Self::command_for(&self.effect_request())]
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
        let normalized = self.normalize_colors(colors);
        let color = normalized.first().copied().unwrap_or([0, 0, 0]);

        let mut buffer = CommandBuffer::new(commands);
        buffer.push_struct(
            &KrakenRequest::ram_write(CUSTOM_COLOR_ADDRESS, &color),
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
        buffer.push_struct(
            &self.effect_request(),
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
        buffer.finish();
    }

    fn connection_diagnostics(&self) -> Vec<ProtocolCommand> {
        let request = KrakenRequest::eeprom_read(FIRMWARE_VERSION_ADDRESS, FIRMWARE_VERSION_LEN);
        vec![
            ProtocolCommand {
                expects_response: true,
                ..Self::command_for(&request)
            }
            .with_response_timeout(DIAGNOSTIC_RESPONSE_TIMEOUT),
        ]
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        let (response, _remainder) = KrakenResponse::read_from_prefix(data).map_err(|_| {
            ProtocolError::MalformedResponse {
                detail: format!(
                    "expected a {KRAKEN_RESPONSE_LEN}-byte Kraken input report, got {} bytes",
                    data.len()
                ),
            }
        })?;

        if response.report_id != KRAKEN_INPUT_REPORT_ID {
            return Err(ProtocolError::MalformedResponse {
                detail: format!(
                    "expected Kraken input report 0x{KRAKEN_INPUT_REPORT_ID:02X}, got 0x{:02X}",
                    response.report_id
                ),
            });
        }

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: response.arguments.to_vec(),
        })
    }

    fn zones(&self) -> Vec<SegmentInfo> {
        vec![SegmentInfo {
            name: "Earcups".to_owned(),
            led_count: self.total_leds(),
            topology: DeviceTopologyHint::Point,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }]
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let max_fps = u32::try_from(1_000 / FRAME_INTERVAL.as_millis()).unwrap_or(u32::MAX);

        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps,
            color_space: DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        1
    }

    fn frame_interval(&self) -> Duration {
        FRAME_INTERVAL
    }
}
