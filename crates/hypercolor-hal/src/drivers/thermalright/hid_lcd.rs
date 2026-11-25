//! The Thermalright HID LCD (`0x0416:0x5302`, spec 96).
//!
//! The panel enumerates as a composite device. Interface 0 is a HID function
//! with 512-byte reports (interrupt OUT `0x02`, IN `0x81`) and carries
//! everything here; interface 1 is a vendor-class function that Windows
//! binds to WinUSB and nothing drives.
//!
//! A session opens with one handshake report. The reply names the panel
//! model (the PM byte) and a sub-model (SUB), and the model picks the pixel
//! byte order. A frame is a 20-byte header followed by the RGB565
//! framebuffer, streamed as 512-byte output reports with the last one
//! zero-padded. The 320x240 glass scans portrait, so the landscape image is
//! turned a quarter clockwise into a 240x320 raster on the way out.
//!
//! The panel falls back to its own logo once frames stop, so the frame pump
//! resends the last frame after a second without one.
//!
//! Units on firmware 4.07 are never claimed (see the descriptor): some of
//! them reboot on the handshake and wedge when reopened.

use std::sync::{Mutex, PoisonError, RwLock};
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceTopologyHint, DisplayFrameFormat,
    DisplayFramePayload, SegmentInfo,
};
use tracing::{debug, info, warn};
use zerocopy::byteorder::{LittleEndian, U16, U32};
use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

use crate::display::{
    ChunkCommandPolicy, ChunkContext, DisplayChunkLayout, DisplayEncodeError, LineRepack,
    Packed16Format, QuarterTurn, WireKeepalive, encode_chunked_display_frame,
};
use crate::protocol::{
    FrameBatch, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ResponsePlan,
    ResponseStatus, TransferType,
};
use crate::transport::format_hex_preview;

/// Vendor ID the panel reports (Winbond, shared with Lian Li hardware).
pub const THERMALRIGHT_VENDOR_ID: u16 = 0x0416;
/// The HID LCD, product string `USBDISPLAY`.
pub const THERMALRIGHT_HID_LCD_PID: u16 = 0x5302;
/// Opens every handshake, reply, and frame header.
pub const HID_LCD_MAGIC: [u8; 4] = [0xDA, 0xDB, 0xDC, 0xDD];
/// Bytes in one HID report on the wire.
pub const HID_LCD_REPORT_LEN: usize = 512;
/// The panel's reports are unnumbered; the host HID stack takes a zero
/// report ID ahead of every output report.
pub const HID_LCD_REPORT_ID: u8 = 0x00;
/// One output report as the host HID stack takes it: the report ID byte,
/// then the 512 wire bytes.
pub const HID_LCD_PACKET_LEN: usize = HID_LCD_REPORT_LEN + 1;
/// Bytes of frame header ahead of the pixels.
pub const HID_LCD_FRAME_HEADER_LEN: usize = 20;
/// Bytes of RGB565 framebuffer in one frame: 240x320 pixels, two bytes each.
pub const HID_LCD_PIXEL_BYTES: usize = 153_600;
/// Output reports one frame takes, the last one zero-padded.
pub const HID_LCD_FRAME_REPORTS: usize =
    (HID_LCD_FRAME_HEADER_LEN + HID_LCD_PIXEL_BYTES).div_ceil(HID_LCD_REPORT_LEN);
/// Silence after which the last frame is sent again. The panel drops to its
/// logo a few seconds after frames stop.
pub const HID_LCD_KEEPALIVE_SILENCE: Duration = Duration::from_secs(1);
/// How often the frame pump checks for that silence.
pub const HID_LCD_PUMP_INTERVAL: Duration = Duration::from_millis(250);

/// Width of the display surface the daemon composes, in pixels.
pub const HID_LCD_WIDTH: u32 = 320;
/// Height of the display surface the daemon composes, in pixels.
pub const HID_LCD_HEIGHT: u32 = 240;
/// Long edge of the glass, in pixels.
const GLASS_LONG_EDGE: u16 = 320;
/// Short edge of the glass, in pixels.
const GLASS_SHORT_EDGE: u16 = 240;
/// `HID_LCD_PIXEL_BYTES` as the frame header's length field carries it.
const PIXEL_BYTES_FIELD: u32 = 153_600;
/// Pause between sending the handshake and reading its reply.
const HANDSHAKE_SETTLE: Duration = Duration::from_millis(200);
/// How long the handshake reply may take; the panel can still be booting.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Settle after the last report of a frame.
const FRAME_SETTLE: Duration = Duration::from_millis(1);
/// Shortest reply the panel identifies itself with.
const MIN_REPLY_LEN: usize = 20;
/// Reply bytes past which a firmware 4.07 streaming unit's short reply is
/// zero: the magic, SUB, and PM.
const SHORT_REPLY_LEN: usize = 6;
/// Marker at reply byte 16 that says a serial follows.
const SERIAL_MARKER: u8 = 0x10;
/// Where a standard reply carries the serial.
const SERIAL_RANGE: std::ops::Range<usize> = 20..36;
/// Reply bytes the debug log shows.
const REPLY_LOG_BYTES: usize = 64;
/// Delivery cadence. The link moves 301 reports per frame; raise this on
/// measurement.
const MAX_FPS: u32 = 30;
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Frame header encoding word for a raw RGB565 framebuffer.
const ENCODING_RGB565: u16 = 1;

const _: () = assert!(
    HID_LCD_PIXEL_BYTES == GLASS_LONG_EDGE as usize * GLASS_SHORT_EDGE as usize * 2,
    "the framebuffer is two bytes per pixel of the glass"
);
const _: () = assert!(
    PIXEL_BYTES_FIELD as usize == HID_LCD_PIXEL_BYTES,
    "the header length field must match the framebuffer"
);
const _: () = assert!(
    HID_LCD_WIDTH == GLASS_LONG_EDGE as u32 && HID_LCD_HEIGHT == GLASS_SHORT_EDGE as u32,
    "the surface is the landscape glass"
);
const _: () = assert!(
    HID_LCD_FRAME_REPORTS == 301,
    "a frame is 300 full reports and one zero-padded report"
);

/// The commands this driver sends. The panel firmware knows more (logo,
/// rotation, backlight, firmware update); none of them is ever encoded here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HidLcdCommand {
    /// Identify the panel. The handshake carries it at report byte 12 and
    /// the reply echoes it there.
    DeviceInfo = 0x01,
    /// One picture. The frame header carries it at bytes 4 and 12.
    Picture = 0x02,
}

impl HidLcdCommand {
    /// The command code on the wire.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// The handshake, as one output report with its report ID in front.
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct HandshakeReport {
    report_id: u8,
    magic: [u8; 4],
    reserved: [u8; 8],
    command: U32<LittleEndian>,
    padding: [u8; 496],
}

const _: () = assert!(
    std::mem::size_of::<HandshakeReport>() == HID_LCD_PACKET_LEN,
    "HandshakeReport must be one report ID byte plus one 512-byte report"
);

/// The header ahead of every frame's pixels, inside the report stream.
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct FrameHeader {
    magic: [u8; 4],
    frame_type: U16<LittleEndian>,
    encoding: U16<LittleEndian>,
    width: U16<LittleEndian>,
    height: U16<LittleEndian>,
    command: U32<LittleEndian>,
    payload_len: U32<LittleEndian>,
}

const _: () = assert!(
    std::mem::size_of::<FrameHeader>() == HID_LCD_FRAME_HEADER_LEN,
    "FrameHeader must be 20 bytes"
);

/// Every RGB565 frame carries this header. The geometry is the wire
/// raster's, 240x320, whichever way the glass is mounted.
const FRAME_HEADER: FrameHeader = FrameHeader {
    magic: HID_LCD_MAGIC,
    frame_type: U16::new(HidLcdCommand::Picture as u16),
    encoding: U16::new(ENCODING_RGB565),
    width: U16::new(GLASS_SHORT_EDGE),
    height: U16::new(GLASS_LONG_EDGE),
    command: U32::new(HidLcdCommand::Picture as u32),
    payload_len: U32::new(PIXEL_BYTES_FIELD),
};

/// A handshake reply the panel accepted the session with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandshakeReply {
    /// Panel model (reply byte 5).
    pub pm: u8,
    /// Sub-model (reply byte 4).
    pub sub: u8,
    /// Serial from reply bytes 20 to 35, when byte 16 announces one.
    pub serial: Option<[u8; 16]>,
}

/// Parse a handshake reply.
///
/// # Errors
///
/// Returns [`ProtocolError::MalformedResponse`] when the reply does not
/// open with [`HID_LCD_MAGIC`], is shorter than a full reply, or does not
/// echo the device-info command at byte 12. The short reply of a firmware
/// 4.07 streaming unit is refused with its own message.
pub fn parse_handshake_reply(data: &[u8]) -> Result<HandshakeReply, ProtocolError> {
    if data.get(..HID_LCD_MAGIC.len()) != Some(HID_LCD_MAGIC.as_slice()) {
        return Err(ProtocolError::MalformedResponse {
            detail: format!(
                "Thermalright LCD handshake reply opens with {}, not the DA DB DC DD magic",
                format_hex_preview(data, HID_LCD_MAGIC.len())
            ),
        });
    }

    let echoed = data.len() >= MIN_REPLY_LEN && data[12] == HidLcdCommand::DeviceInfo.code();
    if !echoed {
        let short =
            data.len() >= SHORT_REPLY_LEN && data[SHORT_REPLY_LEN..].iter().all(|byte| *byte == 0);
        let detail = if short {
            format!(
                "Thermalright LCD answered with the short handshake of the firmware 4.07 \
                 streaming variant (panel model {}, sub {}), which this driver does not drive",
                data[5], data[4]
            )
        } else {
            format!(
                "Thermalright LCD handshake reply is {} bytes and does not echo the \
                 device-info command at byte 12",
                data.len()
            )
        };
        return Err(ProtocolError::MalformedResponse { detail });
    }

    let serial = (data[16] == SERIAL_MARKER)
        .then(|| data.get(SERIAL_RANGE))
        .flatten()
        .and_then(|serial| <[u8; 16]>::try_from(serial).ok());

    Ok(HandshakeReply {
        pm: data[5],
        sub: data[4],
        serial,
    })
}

/// Pixel byte order for a panel model, or `None` for a model this driver
/// does not drive. Models 50 to 53 and 58 send little-endian RGB565; the
/// vendor software sends model 49 high byte first.
#[must_use]
pub const fn model_pixel_format(pm: u8) -> Option<Packed16Format> {
    match pm {
        49 => Some(Packed16Format::Rgb565BigEndian),
        50..=53 | 58 => Some(Packed16Format::Rgb565),
        _ => None,
    }
}

/// The landscape surface turned a quarter clockwise into the 240x320 raster.
const fn repack(format: Packed16Format) -> LineRepack<'static> {
    LineRepack {
        width: GLASS_LONG_EDGE as usize,
        height: GLASS_SHORT_EDGE as usize,
        format,
        line_len: GLASS_SHORT_EDGE as usize * 2,
        filler: 0,
        xor_mask: &[],
        turn: QuarterTurn::Clockwise,
    }
}

/// One output report per 512 bytes of the frame stream, report ID first.
struct HidReportLayout;

impl DisplayChunkLayout for HidReportLayout {
    fn packet_len(&self) -> usize {
        HID_LCD_PACKET_LEN
    }

    fn max_payload(&self) -> usize {
        HID_LCD_REPORT_LEN
    }

    fn payload_offset(&self) -> usize {
        1
    }

    fn write_header(&self, packet: &mut [u8], _ctx: &ChunkContext<'_>) {
        packet[0] = HID_LCD_REPORT_ID;
    }

    fn command_policy(&self, ctx: &ChunkContext<'_>) -> ChunkCommandPolicy {
        ChunkCommandPolicy {
            post_delay: ctx.is_final.then_some(FRAME_SETTLE),
            ..ChunkCommandPolicy::fire_and_forget(TransferType::Primary)
        }
    }

    fn max_chunks(&self) -> u32 {
        // The header declares the payload length; a stream past one frame
        // is a bug, not a bigger panel.
        u32::try_from(HID_LCD_FRAME_REPORTS).unwrap_or(u32::MAX)
    }
}

/// The Thermalright HID LCD protocol.
pub struct HidLcdProtocol {
    /// The reply the session was accepted with.
    panel: RwLock<Option<HandshakeReply>>,
    /// The last frame's report stream: header, then pixels. Empty until a
    /// frame encodes, and what the frame pump resends.
    stream: Mutex<Vec<u8>>,
    keepalive: WireKeepalive,
}

impl Default for HidLcdProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl HidLcdProtocol {
    /// A protocol for a panel that has not been identified yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            panel: RwLock::new(None),
            stream: Mutex::new(Vec::new()),
            keepalive: WireKeepalive::new(HID_LCD_KEEPALIVE_SILENCE),
        }
    }

    /// The reply the panel identified itself with, once it has.
    #[must_use]
    pub fn handshake(&self) -> Option<HandshakeReply> {
        *self.panel.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// The pixel byte order in force: the identified panel's, or
    /// little-endian RGB565, the most common, before the handshake.
    #[must_use]
    pub fn pixel_format(&self) -> Packed16Format {
        self.handshake()
            .and_then(|reply| model_pixel_format(reply.pm))
            .unwrap_or(Packed16Format::Rgb565)
    }

    fn handshake_command() -> ProtocolCommand {
        let mut report = HandshakeReport::new_zeroed();
        report.report_id = HID_LCD_REPORT_ID;
        report.magic = HID_LCD_MAGIC;
        report.command = U32::new(u32::from(HidLcdCommand::DeviceInfo.code()));
        ProtocolCommand {
            data: report.as_bytes().to_vec(),
            expects_response: true,
            response_delay: HANDSHAKE_SETTLE,
            transfer_type: TransferType::Primary,
            response: ResponsePlan {
                timeout: Some(HANDSHAKE_TIMEOUT),
                ..ResponsePlan::default()
            },
            ..ProtocolCommand::default()
        }
    }

    /// Write the header and the packed pixels into `stream`.
    fn fill_stream(
        format: Packed16Format,
        pixels: &[u8],
        stream: &mut Vec<u8>,
    ) -> Result<(), DisplayEncodeError> {
        stream.resize(HID_LCD_FRAME_HEADER_LEN + HID_LCD_PIXEL_BYTES, 0);
        stream[..HID_LCD_FRAME_HEADER_LEN].copy_from_slice(FRAME_HEADER.as_bytes());
        repack(format).repack_rgb888_into(pixels, &mut stream[HID_LCD_FRAME_HEADER_LEN..])?;
        Ok(())
    }
}

impl Protocol for HidLcdProtocol {
    fn name(&self) -> &'static str {
        "Thermalright LCD"
    }

    /// One handshake report; its reply identifies the panel (spec 96
    /// section 5.1).
    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        *self.panel.write().unwrap_or_else(PoisonError::into_inner) = None;
        self.stream
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        vec![Self::handshake_command()]
    }

    /// Nothing: the panel returns to its own logo once frames stop.
    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, _colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame_into(
        &self,
        _colors: &[[u8; 3]],
        commands: &mut Vec<ProtocolCommand>,
    ) -> FrameBatch {
        // The panel has no LEDs.
        commands.clear();
        FrameBatch::from_commands(commands)
    }

    fn frame_pump_interval(&self) -> Option<Duration> {
        Some(HID_LCD_PUMP_INTERVAL)
    }

    /// Resend the last frame once a second has passed without one, so the
    /// panel keeps it instead of falling back to its logo.
    fn pump_frame_into(&self, commands: &mut Vec<ProtocolCommand>) {
        commands.clear();
        if !self.keepalive.due() {
            return;
        }
        let stream = self.stream.lock().unwrap_or_else(PoisonError::into_inner);
        if stream.is_empty() {
            return;
        }
        match encode_chunked_display_frame(&HidReportLayout, &stream, commands) {
            Ok(()) => self.keepalive.mark_sent(),
            Err(error) => {
                warn!(%error, "could not frame the last Thermalright LCD frame for resend");
                commands.clear();
            }
        }
    }

    /// The only reply is the handshake's. It identifies the panel model,
    /// and a model outside the supported table fails the session.
    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        debug!(
            reply_len = data.len(),
            reply = %format_hex_preview(data, REPLY_LOG_BYTES),
            "Thermalright LCD handshake reply"
        );
        let reply = parse_handshake_reply(data)?;
        let Some(format) = model_pixel_format(reply.pm) else {
            warn!(
                pm = reply.pm,
                sub = reply.sub,
                "Thermalright LCD reports a panel model this driver does not drive yet; \
                 only the 320x240 panels (models 49 to 53 and 58) are supported, so the \
                 panel is left alone"
            );
            return Err(ProtocolError::DeviceError {
                status: ResponseStatus::Unsupported,
            });
        };

        let serial = reply
            .serial
            .map(|serial| format_hex_preview(&serial, serial.len()));
        info!(
            pm = reply.pm,
            sub = reply.sub,
            format = ?format,
            serial = serial.as_deref().unwrap_or("<none>"),
            "Thermalright LCD identified"
        );
        *self.panel.write().unwrap_or_else(PoisonError::into_inner) = Some(reply);

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: vec![reply.pm, reply.sub],
        })
    }

    fn encode_display_payload_into(
        &self,
        payload: DisplayFramePayload<'_>,
        commands: &mut Vec<ProtocolCommand>,
    ) -> Result<(), DisplayEncodeError> {
        if payload.format != DisplayFrameFormat::Rgb {
            return Err(DisplayEncodeError::Unsupported {
                format: payload.format,
            });
        }
        if payload.width != HID_LCD_WIDTH || payload.height != HID_LCD_HEIGHT {
            return Err(DisplayEncodeError::WrongGeometry {
                expected_width: HID_LCD_WIDTH,
                expected_height: HID_LCD_HEIGHT,
                width: payload.width,
                height: payload.height,
            });
        }

        let mut stream = self.stream.lock().unwrap_or_else(PoisonError::into_inner);
        let framed = Self::fill_stream(self.pixel_format(), payload.data, &mut stream)
            .and_then(|()| encode_chunked_display_frame(&HidReportLayout, &stream, commands));
        if framed.is_ok() {
            self.keepalive.mark_sent();
        } else {
            // A frame that did not encode is never resent.
            stream.clear();
        }
        framed
    }

    fn zones(&self) -> Vec<SegmentInfo> {
        vec![SegmentInfo {
            name: "Display".to_owned(),
            led_count: 0,
            topology: DeviceTopologyHint::Display {
                width: HID_LCD_WIDTH,
                height: HID_LCD_HEIGHT,
                circular: false,
                format: DisplayFrameFormat::Rgb,
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
            features: DeviceFeatures::default(),
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
