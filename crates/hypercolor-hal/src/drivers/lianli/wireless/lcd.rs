//! The wired LCD of a wireless Lian Li fan: a USB bulk receiver
//! (`0x1CBE:0x0006` for TL, `0x1CBE:0x0005` for SL V3) that takes
//! DES-wrapped commands and a fixed 102,400-byte frame write (spec 80
//! section 7).
//!
//! Every command is one 512-byte encrypted header; a frame is that header
//! followed by the JPEG in a single fixed-size bulk write. The receiver
//! answers most commands with a status packet whose layout is undocumented
//! and which it sometimes skips, so every read after a command is optional
//! and the reply is only inspected for the firmware string.

use std::sync::{Mutex, PoisonError, RwLock};
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceTopologyHint, DisplayFrameFormat,
    DisplayFramePayload, SegmentInfo,
};

use super::crypto::{HEADER_LEN, HeaderBuilder};
use crate::display::{ChunkCommandPolicy, DisplayEncodeError, encode_prefixed_display_frame_into};
use crate::drivers::lianli::common::nul_terminated_ascii;
use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ResponsePlan,
    ResponseStatus, ResponseTolerance, TransferType,
};

/// Vendor ID the receivers borrow (Luminary Micro / TI).
pub const WIRELESS_LCD_VENDOR_ID: u16 = 0x1CBE;
/// The TL V2 wireless LCD receiver.
pub const PID_TL_WIRELESS_LCD: u16 = 0x0006;
/// The SL V3 wireless LCD receiver, same protocol.
pub const PID_SL_WIRELESS_LCD: u16 = 0x0005;
/// Panel resolution, square and round.
pub const WIRELESS_LCD_RESOLUTION: u32 = 400;
/// Every frame write is exactly this long.
pub const WIRELESS_LCD_FRAME_LEN: usize = 102_400;
/// JPEG bytes a frame can carry after its header.
pub const WIRELESS_LCD_MAX_JPEG_LEN: usize = WIRELESS_LCD_FRAME_LEN - HEADER_LEN;
/// Bytes a status reply is read into.
const STATUS_REPLY_CAPACITY: usize = 511;
/// Frame rate byte the receiver is set to at init; what the reference sends.
const INIT_FRAME_RATE: u8 = 120;
/// Hardware brightness at init, on the receiver's 0..=100 scale before the
/// firmware LUT; the daemon's software brightness is the runtime authority.
const INIT_BRIGHTNESS_PERCENT: u8 = 100;
/// Rotation byte for the unrotated panel; orientation is a layout property.
const INIT_ROTATION: u8 = 0;
/// Reads during init, where the receiver is slow to answer.
const INIT_TIMEOUT: Duration = Duration::from_secs(2);
/// The status read after a frame; a stalled reply must not hold the lane.
const STEADY_TIMEOUT: Duration = Duration::from_millis(200);
/// Delivery cadence baseline: the panel refreshes at 60 Hz, the daemon's
/// JPEG caps govern what it actually receives; raise on measurement.
const MAX_FPS: u32 = 30;
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Command byte at plaintext offset 0 (section 7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WirelessLcdCommand {
    /// Read the firmware version; the reply carries it at offset 8.
    GetVer = 0x0A,
    /// Set the rotation, 0 to 3 for 0/90/180/270 degrees.
    Rotate = 0x0D,
    /// Set the backlight, through the firmware's lookup table.
    Brightness = 0x0E,
    /// Set the refresh rate.
    FrameRate = 0x0F,
    /// Push one JPEG frame; parameters are its size, big-endian.
    PushJpg = 0x65,
    /// Probe the hardware revision, the first thing a session sends.
    CheckNewLcd = 0x80,
}

/// The firmware's brightness curve: five anchors, linear between them.
#[must_use]
pub fn brightness_lut(percent: u8) -> u8 {
    const ANCHORS: [(u8, u8); 5] = [(0, 0), (25, 10), (50, 30), (75, 40), (100, 100)];
    let percent = percent.min(100);
    let Some(upper) = ANCHORS.iter().position(|(input, _)| *input >= percent) else {
        return 100;
    };
    let (hi_in, hi_out) = ANCHORS[upper];
    if hi_in == percent || upper == 0 {
        return hi_out;
    }
    let (lo_in, lo_out) = ANCHORS[upper - 1];
    let span = u32::from(hi_in - lo_in);
    let offset = u32::from(percent - lo_in);
    let rise = u32::from(hi_out) - u32::from(lo_out);
    let step = (offset * rise + span / 2) / span;
    u8::try_from(u32::from(lo_out) + step).unwrap_or(u8::MAX)
}

/// The wireless LCD receiver protocol.
pub struct WirelessLcdProtocol {
    headers: Mutex<HeaderBuilder>,
    firmware: RwLock<Option<String>>,
}

impl Default for WirelessLcdProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl WirelessLcdProtocol {
    /// A receiver protocol with a fresh timestamp clock.
    #[must_use]
    pub fn new() -> Self {
        Self {
            headers: Mutex::new(HeaderBuilder::new()),
            firmware: RwLock::new(None),
        }
    }

    /// Firmware string from the GetVer reply, once seen.
    #[must_use]
    pub fn firmware(&self) -> Option<String> {
        self.firmware
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn header(&self, command: WirelessLcdCommand, params: &[u8]) -> [u8; HEADER_LEN] {
        self.headers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .header(command as u8, params)
    }

    /// The optional status read that follows every command but a reboot.
    const fn status_read(timeout: Duration) -> ResponsePlan {
        ResponsePlan {
            count: 1,
            timeout: Some(timeout),
            capacity: Some(STATUS_REPLY_CAPACITY),
            tolerance: ResponseTolerance::Optional,
        }
    }

    fn control(
        &self,
        command: WirelessLcdCommand,
        params: &[u8],
        timeout: Duration,
    ) -> ProtocolCommand {
        ProtocolCommand {
            data: self.header(command, params).to_vec(),
            expects_response: true,
            response: Self::status_read(timeout),
            ..Default::default()
        }
    }
}

impl Protocol for WirelessLcdProtocol {
    fn name(&self) -> &'static str {
        "Lian Li Uni Fan Wireless LCD"
    }

    /// Probe the revision, set the refresh rate, read the firmware, then
    /// put the backlight and rotation in a known state.
    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        *self
            .firmware
            .write()
            .unwrap_or_else(PoisonError::into_inner) = None;
        vec![
            self.control(WirelessLcdCommand::CheckNewLcd, &[], INIT_TIMEOUT),
            self.control(
                WirelessLcdCommand::FrameRate,
                &[INIT_FRAME_RATE],
                INIT_TIMEOUT,
            ),
            self.control(WirelessLcdCommand::GetVer, &[], INIT_TIMEOUT),
            self.control(
                WirelessLcdCommand::Brightness,
                &[brightness_lut(INIT_BRIGHTNESS_PERCENT)],
                INIT_TIMEOUT,
            ),
            self.control(WirelessLcdCommand::Rotate, &[INIT_ROTATION], INIT_TIMEOUT),
        ]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, _colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        // The receiver drives a display; the fan's LEDs belong to the
        // wireless controller.
        Vec::new()
    }

    fn encode_display_payload_into(
        &self,
        payload: DisplayFramePayload<'_>,
        commands: &mut Vec<ProtocolCommand>,
    ) -> Result<(), DisplayEncodeError> {
        if payload.format != DisplayFrameFormat::Jpeg {
            return Err(DisplayEncodeError::Unsupported {
                format: payload.format,
            });
        }
        let size = u32::try_from(payload.data.len()).unwrap_or(u32::MAX);
        let header = self.header(WirelessLcdCommand::PushJpg, &size.to_be_bytes());
        let policy = ChunkCommandPolicy {
            transfer_type: TransferType::Primary,
            expects_response: true,
            response_delay: Duration::ZERO,
            post_delay: None,
            response: Self::status_read(STEADY_TIMEOUT),
        };

        let mut buffer = CommandBuffer::new(commands);
        let framed = encode_prefixed_display_frame_into(
            HEADER_LEN,
            |frame, _ctx| frame[..HEADER_LEN].copy_from_slice(&header),
            payload.data,
            Some(WIRELESS_LCD_FRAME_LEN),
            policy,
            &mut buffer,
        );
        buffer.finish();
        framed
    }

    /// Status replies are undocumented and ignored; a GetVer reply carries
    /// the firmware string as NUL-padded ASCII from offset 8.
    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        if data.len() > 8 && data[0] == WirelessLcdCommand::GetVer as u8 {
            let firmware = nul_terminated_ascii(&data[8..]);
            if !firmware.is_empty() {
                *self
                    .firmware
                    .write()
                    .unwrap_or_else(PoisonError::into_inner) = Some(firmware);
            }
        }
        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data.to_vec(),
        })
    }

    fn response_timeout(&self) -> Duration {
        STEADY_TIMEOUT
    }

    fn zones(&self) -> Vec<SegmentInfo> {
        vec![SegmentInfo {
            name: "Display".to_owned(),
            led_count: 0,
            topology: DeviceTopologyHint::Display {
                width: WIRELESS_LCD_RESOLUTION,
                height: WIRELESS_LCD_RESOLUTION,
                circular: true,
                format: DisplayFrameFormat::Jpeg,
            },
            // LED byte order has no meaning for a display segment.
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }]
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: 0,
            supports_direct: false,
            supports_brightness: false,
            max_fps: MAX_FPS,
            features: DeviceFeatures {
                max_display_frame_len: Some(WIRELESS_LCD_MAX_JPEG_LEN),
                ..DeviceFeatures::default()
            },
            ..DeviceCapabilities::default()
        }
    }

    fn total_leds(&self) -> u32 {
        0
    }

    fn frame_interval(&self) -> Duration {
        FRAME_INTERVAL
    }
}
