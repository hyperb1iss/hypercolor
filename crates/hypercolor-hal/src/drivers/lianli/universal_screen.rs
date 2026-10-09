//! The Lian Li Universal Screen 8.8" (`0x1CBE:0xA088`, spec 84): a
//! 1920x480 bar LCD whose controller takes JPEG frames over USB bulk behind
//! a DES-wrapped WinUSB header ([`super::winusb`]).
//!
//! The panel is mounted turned a quarter inside its frame, so the firmware
//! only decodes 480x1920 portrait images and silently drops anything else:
//! no acknowledgement, no error, the vendor idle animation keeps playing.
//! The display surface is declared portrait for that reason; the mount
//! rotation the user sets turns content to read upright.
//!
//! A frame is one bulk write: the `PushJpg` header followed by the JPEG at
//! its natural length. Every command, frames included, is answered with a
//! plaintext status packet (command echo at byte 0, `0xC8` at byte 1 for
//! success); the host reads it to keep the IN pipe drained and otherwise
//! tolerates its absence. The LED ring around the panel is a separate USB
//! device and is not driven here.

use std::io::Cursor;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, Mutex, PoisonError, RwLock};
use std::time::Duration;

use chrono::{Datelike, Local, Timelike};
use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceTopologyHint, DisplayFrameFormat,
    DisplayFramePayload, SegmentInfo,
};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use tracing::{debug, warn};

use super::common::nul_terminated_ascii;
use super::winusb::{WINUSB_HEADER_LEN, WinUsbHeaderBuilder};
use crate::display::{ChunkCommandPolicy, DisplayEncodeError, encode_prefixed_display_frame_into};
use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ResponsePlan,
    ResponseStatus, ResponseTolerance, TransferType,
};

/// Vendor ID the panel borrows (Luminary Micro / TI).
pub const UNIVERSAL_SCREEN_VENDOR_ID: u16 = 0x1CBE;
/// The Universal Screen 8.8" in LCD mode. In desktop mode the panel
/// re-enumerates as a WCH USB display device instead.
pub const PID_UNIVERSAL_SCREEN_88: u16 = 0xA088;
/// Width of the image the firmware decodes, in pixels.
pub const UNIVERSAL_SCREEN_WIDTH: u32 = 480;
/// Height of the image the firmware decodes, in pixels.
pub const UNIVERSAL_SCREEN_HEIGHT: u32 = 1920;
/// Largest JPEG one frame write carries.
pub const UNIVERSAL_SCREEN_MAX_JPEG_LEN: usize = 512_000;
/// Status byte at reply offset 1 for an accepted command.
pub const REPLY_STATUS_OK: u8 = 0xC8;
/// Buffered-frame count past which the reference driver waits for the
/// panel to drain before sending again.
pub const BUFFER_HIGH_WATER: u8 = 3;

/// Bytes a status reply is read into; replies fit one bulk packet.
const REPLY_CAPACITY: usize = 512;
/// Where a `GetVer` reply's firmware string sits.
const FIRMWARE_RANGE: std::ops::Range<usize> = 8..40;
/// Frame rate byte sent at init; what the reference sends.
const INIT_FRAME_RATE: u8 = 120;
/// Hardware backlight at init, on the panel's 0..=100 scale. The daemon's
/// software brightness is the runtime authority.
const INIT_BRIGHTNESS_PERCENT: u8 = 100;
/// `SetClock` mode byte the reference sends before stopping the clock.
const CLOCK_MODE: u8 = 2;
/// Quality of the black frame that clears the JPEG layer at init.
const CLEAR_JPEG_QUALITY: u8 = 95;
/// Reads during init, where the panel is slow to answer.
const INIT_TIMEOUT: Duration = Duration::from_secs(2);
/// Pause after `StopPlay` so the panel winds down playback before the next
/// command; the reference waits this long after it.
const STOP_PLAY_SETTLE: Duration = Duration::from_millis(150);
/// The status read after a frame; a stalled reply must not hold the lane.
const STEADY_TIMEOUT: Duration = Duration::from_millis(200);
/// Delivery cadence baseline. The firmware accepts up to 120 Hz; the
/// daemon's JPEG budget governs what it actually receives. Raise on
/// measurement.
const MAX_FPS: u32 = 30;
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Command byte at plaintext offset 0 (spec 84 section 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum UniversalScreenCommand {
    /// Read the firmware version; the reply carries it at offset 8.
    GetVer = 0x0A,
    /// Set the backlight, 0 to 100.
    Brightness = 0x0E,
    /// Set the refresh rate.
    FrameRate = 0x0F,
    /// Set the on-panel clock: year (big-endian), month, day, hour,
    /// minute, second, mode.
    SetClock = 0x33,
    /// Stop the on-panel clock overlay.
    StopClock = 0x34,
    /// Push one frame to the opaque JPEG layer; parameters are its size,
    /// big-endian. The live path.
    PushJpg = 0x65,
    /// Push an image to the PNG overlay composited above the JPEG layer.
    PushPng = 0x66,
    /// Stop H.264 playback left running by another host.
    StopPlay = 0x7B,
}

/// `SetClock` parameters for a local wall-clock time.
#[must_use]
pub fn clock_params<T: Datelike + Timelike>(now: &T) -> [u8; 8] {
    let year = u16::try_from(now.year()).unwrap_or(0).to_be_bytes();
    let byte = |value: u32| u8::try_from(value).unwrap_or(0);
    [
        year[0],
        year[1],
        byte(now.month()),
        byte(now.day()),
        byte(now.hour()),
        byte(now.minute()),
        byte(now.second()),
        CLOCK_MODE,
    ]
}

/// A fully transparent portrait PNG, pushed at init to clear any overlay a
/// previous host left on the panel.
static CLEAR_PNG: LazyLock<Option<Vec<u8>>> = LazyLock::new(|| {
    let pixels = vec![0_u8; panel_pixels() * 4];
    let mut png = Vec::new();
    PngEncoder::new(Cursor::new(&mut png))
        .write_image(
            &pixels,
            UNIVERSAL_SCREEN_WIDTH,
            UNIVERSAL_SCREEN_HEIGHT,
            ExtendedColorType::Rgba8,
        )
        .inspect_err(|error| warn!(%error, "could not encode the overlay-clearing PNG"))
        .ok()?;
    Some(png)
});

/// A black portrait JPEG, pushed at init so the panel leaves the vendor
/// idle animation before the first frame arrives.
static CLEAR_JPEG: LazyLock<Option<Vec<u8>>> = LazyLock::new(|| {
    let pixels = vec![0_u8; panel_pixels() * 3];
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(Cursor::new(&mut jpeg), CLEAR_JPEG_QUALITY)
        .write_image(
            &pixels,
            UNIVERSAL_SCREEN_WIDTH,
            UNIVERSAL_SCREEN_HEIGHT,
            ExtendedColorType::Rgb8,
        )
        .inspect_err(|error| warn!(%error, "could not encode the layer-clearing JPEG"))
        .ok()?;
    Some(jpeg)
});

fn panel_pixels() -> usize {
    usize::try_from(UNIVERSAL_SCREEN_WIDTH * UNIVERSAL_SCREEN_HEIGHT).unwrap_or(0)
}

/// The Universal Screen 8.8" protocol.
pub struct UniversalScreenProtocol {
    headers: Mutex<WinUsbHeaderBuilder>,
    firmware: RwLock<Option<String>>,
    buffer_level: AtomicU8,
}

impl Default for UniversalScreenProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl UniversalScreenProtocol {
    /// A panel protocol with a fresh timestamp clock.
    #[must_use]
    pub fn new() -> Self {
        Self {
            headers: Mutex::new(WinUsbHeaderBuilder::new()),
            firmware: RwLock::new(None),
            buffer_level: AtomicU8::new(0),
        }
    }

    /// Firmware string from the `GetVer` reply, once seen.
    #[must_use]
    pub fn firmware(&self) -> Option<String> {
        self.firmware
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Frames the panel reported buffered in its last frame acknowledgement.
    #[must_use]
    pub fn last_buffer_level(&self) -> u8 {
        self.buffer_level.load(Ordering::Relaxed)
    }

    fn header(&self, command: UniversalScreenCommand, params: &[u8]) -> [u8; WINUSB_HEADER_LEN] {
        self.headers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .header(command as u8, params)
    }

    /// The optional status read that follows every command.
    const fn status_read(timeout: Duration) -> ResponsePlan {
        ResponsePlan {
            count: 1,
            timeout: Some(timeout),
            capacity: Some(REPLY_CAPACITY),
            tolerance: ResponseTolerance::Optional,
        }
    }

    fn control(&self, command: UniversalScreenCommand, params: &[u8]) -> ProtocolCommand {
        ProtocolCommand {
            data: self.header(command, params).to_vec(),
            expects_response: true,
            response: Self::status_read(INIT_TIMEOUT),
            ..Default::default()
        }
    }

    /// An image pushed at init: the header and the image in one write.
    fn layer(&self, command: UniversalScreenCommand, image: &[u8]) -> Option<ProtocolCommand> {
        let size = u32::try_from(image.len()).ok()?;
        let mut data = Vec::with_capacity(WINUSB_HEADER_LEN + image.len());
        data.extend_from_slice(&self.header(command, &size.to_be_bytes()));
        data.extend_from_slice(image);
        Some(ProtocolCommand {
            data,
            expects_response: true,
            response: Self::status_read(INIT_TIMEOUT),
            ..Default::default()
        })
    }

    fn record_firmware(&self, reply: &[u8]) {
        let Some(field) = reply.get(FIRMWARE_RANGE).or_else(|| reply.get(8..)) else {
            return;
        };
        let firmware = nul_terminated_ascii(field);
        if firmware.is_empty()
            || !firmware
                .bytes()
                .all(|byte| byte.is_ascii_graphic() || byte == b' ')
        {
            return;
        }
        debug!(%firmware, "Universal Screen firmware");
        *self
            .firmware
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(firmware);
    }

    fn record_buffer_level(&self, reply: &[u8]) {
        let Some(&level) = reply.get(8) else {
            return;
        };
        let previous = self.buffer_level.swap(level, Ordering::Relaxed);
        if level > BUFFER_HIGH_WATER && previous <= BUFFER_HIGH_WATER {
            debug!(
                level,
                high_water = BUFFER_HIGH_WATER,
                "Universal Screen frame buffer is filling"
            );
        }
    }
}

impl Protocol for UniversalScreenProtocol {
    fn name(&self) -> &'static str {
        "Lian Li Universal Screen 8.8\""
    }

    /// Stop any playback another host left running, read the firmware,
    /// settle the panel, stop the clock overlay, then clear both image
    /// layers (spec 84 section 5).
    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        *self
            .firmware
            .write()
            .unwrap_or_else(PoisonError::into_inner) = None;
        self.buffer_level.store(0, Ordering::Relaxed);

        let mut commands = vec![
            ProtocolCommand {
                post_delay: STOP_PLAY_SETTLE,
                ..self.control(UniversalScreenCommand::StopPlay, &[])
            },
            self.control(UniversalScreenCommand::GetVer, &[]),
            self.control(UniversalScreenCommand::FrameRate, &[INIT_FRAME_RATE]),
            self.control(
                UniversalScreenCommand::Brightness,
                &[INIT_BRIGHTNESS_PERCENT],
            ),
            self.control(
                UniversalScreenCommand::SetClock,
                &clock_params(&Local::now()),
            ),
            self.control(UniversalScreenCommand::StopClock, &[0]),
        ];
        commands.extend(
            CLEAR_PNG
                .as_deref()
                .and_then(|png| self.layer(UniversalScreenCommand::PushPng, png)),
        );
        commands.extend(
            CLEAR_JPEG
                .as_deref()
                .and_then(|jpeg| self.layer(UniversalScreenCommand::PushJpg, jpeg)),
        );
        commands
    }

    /// The panel keeps its last frame; nothing hands it back to a vendor
    /// mode short of a reboot, which would drop it into desktop mode.
    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, _colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame_into(&self, _colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        // The panel has no LEDs; its ring is a separate USB device.
        commands.clear();
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
        let too_large = DisplayEncodeError::PayloadTooLarge {
            actual: payload.data.len(),
            capacity: UNIVERSAL_SCREEN_MAX_JPEG_LEN,
        };
        if payload.data.len() > UNIVERSAL_SCREEN_MAX_JPEG_LEN {
            return Err(too_large);
        }
        let size = u32::try_from(payload.data.len()).map_err(|_| too_large)?;
        let header = self.header(UniversalScreenCommand::PushJpg, &size.to_be_bytes());
        let policy = ChunkCommandPolicy {
            transfer_type: TransferType::Primary,
            expects_response: true,
            response_delay: Duration::ZERO,
            post_delay: None,
            response: Self::status_read(STEADY_TIMEOUT),
        };

        let mut buffer = CommandBuffer::new(commands);
        let framed = encode_prefixed_display_frame_into(
            WINUSB_HEADER_LEN,
            |frame, _ctx| frame[..WINUSB_HEADER_LEN].copy_from_slice(&header),
            payload.data,
            None,
            policy,
            &mut buffer,
        );
        buffer.finish();
        framed
    }

    /// Replies are plaintext: the command echoed at byte 0, a status at
    /// byte 1, and a command-specific body from byte 8. `GetVer` carries
    /// the firmware string there and `PushJpg` the panel's buffered-frame
    /// count. Nothing in a reply fails a command; unexpected statuses are
    /// logged for diagnosis.
    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        match data.first().copied() {
            Some(command) if command == UniversalScreenCommand::GetVer as u8 => {
                self.record_firmware(data);
            }
            Some(command) if command == UniversalScreenCommand::PushJpg as u8 => {
                self.record_buffer_level(data);
            }
            _ => {}
        }
        if let (Some(&command), Some(&status)) = (data.first(), data.get(1))
            && status != REPLY_STATUS_OK
        {
            debug!(
                command = format_args!("0x{command:02X}"),
                status = format_args!("0x{status:02X}"),
                "Universal Screen answered with an unexpected status"
            );
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
                width: UNIVERSAL_SCREEN_WIDTH,
                height: UNIVERSAL_SCREEN_HEIGHT,
                circular: false,
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
                max_display_frame_len: Some(UNIVERSAL_SCREEN_MAX_JPEG_LEN),
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
