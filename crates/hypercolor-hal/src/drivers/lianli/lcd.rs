//! Lian Li wired Uni Fan TL LCD panel protocol.
//!
//! A 400x400 round IPS panel behind HID output reports: fixed 512-byte
//! packets carrying an 11-byte header and up to 501 payload bytes. Live
//! frames stream as JPEG over `WriteSyncJpg` with no acknowledgements; the
//! session commands (identity, handshake, firmware, panel settings) each read
//! a reply.

use std::sync::{PoisonError, RwLock};
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceTopologyHint, DisplayFrameFormat,
    DisplayFramePayload, SegmentInfo,
};
use tracing::{debug, warn};
use zerocopy::byteorder::{BigEndian, U16, U32};
use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

use crate::display::{
    ChunkCommandPolicy, ChunkContext, DisplayChunkLayout, DisplayEncodeError, DisplayRotation,
    DisplaySetting, encode_chunked_display_frame,
};
use crate::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ResponseStatus, TransferType,
};

/// HID report ID every panel packet carries.
pub const TL_LCD_REPORT_ID: u8 = 0x02;

/// Fixed on-wire packet size.
pub const TL_LCD_PACKET_LEN: usize = 512;

/// Header bytes ahead of the payload in every packet.
pub const TL_LCD_HEADER_LEN: usize = 11;

/// Payload capacity of one packet.
pub const TL_LCD_MAX_PAYLOAD: usize = TL_LCD_PACKET_LEN - TL_LCD_HEADER_LEN;

/// Chunks one transfer can address.
///
/// The packet counter is 24 bits wide, so this is the whole counter rather
/// than a policy choice.
pub const TL_LCD_MAX_CHUNKS: u32 = 1 << 24;

/// Panel resolution, square and round.
pub const TL_LCD_RESOLUTION: u32 = 400;

const TL_LCD_MAX_FPS: u32 = 30;
const TL_LCD_FRAME_INTERVAL: Duration = Duration::from_millis(33);
/// Reads during a session handshake; the panel is slow to answer identity,
/// handshake, and firmware queries.
const TL_LCD_INIT_TIMEOUT: Duration = Duration::from_secs(3);
/// Reads once streaming, where a stalled reply must not hold the pipeline.
const TL_LCD_STEADY_TIMEOUT: Duration = Duration::from_millis(200);
/// `GetProductInfo` answers with a version report and a build-date report.
const TL_LCD_PRODUCT_INFO_REPORTS: u8 = 2;

const TL_LCD_CONTROL_PAYLOAD_LEN: usize = 11;
const TL_LCD_SERIAL_LEN: usize = 32;
const TL_LCD_DEFAULT_BRIGHTNESS: u8 = 100;
const TL_LCD_DEFAULT_FPS: u8 = 30;

/// Command byte of a panel packet (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TlLcdCommand {
    /// Read the panel's current mode and frame counter.
    GetHandshake = 0x3C,
    /// Read the firmware version, then the build date.
    GetProductInfo = 0x3D,
    /// Read the stored serial plus hub port and chain index.
    ReadSerial = 0x3E,
    /// Write a 32-byte serial. Documented, deliberately never sent: it
    /// mutates user hardware and re-keys the device fingerprint, orphaning
    /// the path-keyed device it just renamed.
    WriteSerial = 0x3F,
    /// Apply panel settings or switch display mode.
    LcdControl = 0x40,
    /// Write a static image, acknowledged per chunk.
    WriteJpg = 0x41,
    /// Stream a live frame, unacknowledged. The live path.
    WriteSyncJpg = 0x46,
}

/// `LcdControl` mode byte (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TlLcdMode {
    /// Display the stored static image.
    ShowJpg = 1,
    /// Play the stored animation.
    ShowAvi = 3,
    /// Display streamed frames.
    ShowAppSync = 4,
    /// Apply brightness, frame rate, and rotation without changing what the
    /// panel is displaying.
    LcdSetting = 5,
    /// Factory test pattern.
    LcdTest = 6,
}

/// Wire-format header of a panel packet (11 bytes).
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct TlLcdHeader {
    /// HID report identifier (always `0x02`).
    report_id: u8,
    /// Command byte.
    command: u8,
    /// Full transfer size in bytes, repeated in every chunk of the transfer.
    total_size: U32<BigEndian>,
    /// Zero-based chunk counter as u24 big-endian, reset per transfer.
    packet_number: [u8; 3],
    /// Payload bytes carried by this packet.
    payload_len: U16<BigEndian>,
}

/// Wire-format panel packet (512 bytes).
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct TlLcdPacket {
    /// Framing header.
    header: TlLcdHeader,
    /// Command payload or JPEG chunk, zero-padded.
    payload: [u8; TL_LCD_MAX_PAYLOAD],
}

const _: () = assert!(
    std::mem::size_of::<TlLcdHeader>() == TL_LCD_HEADER_LEN,
    "TlLcdHeader must match the 11-byte panel packet header"
);

const _: () = assert!(
    std::mem::size_of::<TlLcdPacket>() == TL_LCD_PACKET_LEN,
    "TlLcdPacket must match the 512-byte panel packet size"
);

/// Write a panel packet header into the first bytes of `packet`.
fn write_tl_lcd_header(
    packet: &mut [u8],
    command: TlLcdCommand,
    total_size: u32,
    packet_number: u32,
    payload_len: u16,
) {
    debug_assert!(
        packet.len() >= TL_LCD_HEADER_LEN,
        "panel packet must hold its {TL_LCD_HEADER_LEN}-byte header, got {}B",
        packet.len(),
    );
    if packet.len() < TL_LCD_HEADER_LEN {
        return;
    }

    let [_, counter_hi, counter_mid, counter_lo] = packet_number.to_be_bytes();
    let header = TlLcdHeader {
        report_id: TL_LCD_REPORT_ID,
        command: command as u8,
        total_size: U32::new(total_size),
        packet_number: [counter_hi, counter_mid, counter_lo],
        payload_len: U16::new(payload_len),
    };
    packet[..TL_LCD_HEADER_LEN].copy_from_slice(header.as_bytes());
}

/// Build one complete, zero-padded packet for a non-chunked command.
///
/// A single-packet command is its own transfer, so its counter is zero and
/// its declared total size is just the payload length.
fn build_tl_lcd_packet(command: TlLcdCommand, payload: &[u8]) -> Vec<u8> {
    let payload_len = payload.len().min(TL_LCD_MAX_PAYLOAD);
    let mut packet = vec![0_u8; TL_LCD_PACKET_LEN];
    write_tl_lcd_header(
        &mut packet,
        command,
        u32::try_from(payload_len).unwrap_or(u32::MAX),
        0,
        u16::try_from(payload_len).unwrap_or(u16::MAX),
    );
    packet[TL_LCD_HEADER_LEN..TL_LCD_HEADER_LEN + payload_len]
        .copy_from_slice(&payload[..payload_len]);
    packet
}

/// Panel state learned from replies, plus the settings last asked for.
#[derive(Debug, Clone)]
struct TlLcdState {
    serial: Option<String>,
    port: Option<u8>,
    index: Option<u8>,
    firmware: Option<String>,
    mode: Option<u8>,
    frame_index: Option<u16>,
    brightness: u8,
    fps: u8,
    rotation: DisplayRotation,
}

impl Default for TlLcdState {
    fn default() -> Self {
        Self {
            serial: None,
            port: None,
            index: None,
            firmware: None,
            mode: None,
            frame_index: None,
            brightness: TL_LCD_DEFAULT_BRIGHTNESS,
            fps: TL_LCD_DEFAULT_FPS,
            rotation: DisplayRotation::Deg0,
        }
    }
}

/// Chunk framing for a streamed JPEG frame.
struct TlLcdDisplayLayout {
    command: TlLcdCommand,
}

impl DisplayChunkLayout for TlLcdDisplayLayout {
    fn packet_len(&self) -> usize {
        TL_LCD_PACKET_LEN
    }

    fn max_payload(&self) -> usize {
        TL_LCD_MAX_PAYLOAD
    }

    fn payload_offset(&self) -> usize {
        TL_LCD_HEADER_LEN
    }

    fn write_header(&self, packet: &mut [u8], ctx: &ChunkContext<'_>) {
        write_tl_lcd_header(
            packet,
            self.command,
            u32::try_from(ctx.total_len).unwrap_or(u32::MAX),
            ctx.packet_index,
            u16::try_from(ctx.payload.len()).unwrap_or(u16::MAX),
        );
    }

    fn command_policy(&self, _ctx: &ChunkContext<'_>) -> ChunkCommandPolicy {
        match self.command {
            // The streamed path is unacknowledged: the panel displays frames
            // as they arrive and an ack per chunk would halve the frame rate.
            TlLcdCommand::WriteSyncJpg => {
                ChunkCommandPolicy::fire_and_forget(TransferType::Primary)
            }
            // A stored image is acknowledged chunk by chunk.
            _ => ChunkCommandPolicy {
                transfer_type: TransferType::Primary,
                expects_response: true,
                response_delay: Duration::ZERO,
                post_delay: None,
            },
        }
    }

    fn max_chunks(&self) -> u32 {
        TL_LCD_MAX_CHUNKS
    }
}

/// Wired Uni Fan TL LCD panel protocol.
pub struct TlLcdProtocol {
    state: RwLock<TlLcdState>,
    sync_layout: TlLcdDisplayLayout,
    static_layout: TlLcdDisplayLayout,
}

impl TlLcdProtocol {
    /// Create a wired TL LCD protocol instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: RwLock::new(TlLcdState::default()),
            sync_layout: TlLcdDisplayLayout {
                command: TlLcdCommand::WriteSyncJpg,
            },
            static_layout: TlLcdDisplayLayout {
                command: TlLcdCommand::WriteJpg,
            },
        }
    }

    /// Encode a static image: acknowledged chunks, then the mode switch that
    /// latches them onto the panel.
    ///
    /// The live path is [`Protocol::encode_display_payload_into`]; this is the
    /// stored-image path, which the daemon does not drive today.
    ///
    /// # Errors
    ///
    /// Returns [`DisplayEncodeError`] when the image needs more chunks than
    /// the packet counter can address.
    pub fn encode_static_image(
        &self,
        jpeg_data: &[u8],
    ) -> Result<Vec<ProtocolCommand>, DisplayEncodeError> {
        let mut commands = Vec::new();
        encode_chunked_display_frame(&self.static_layout, jpeg_data, &mut commands)?;
        if !commands.is_empty() {
            commands.push(self.control_command(TlLcdMode::ShowJpg));
        }

        Ok(commands)
    }

    /// Serial reported by the panel, which is a shared placeholder on stock
    /// firmware (§5.7).
    #[must_use]
    pub fn serial(&self) -> Option<String> {
        self.read_state().serial.clone()
    }

    /// Hub port and chain index the panel reported, stable per position.
    #[must_use]
    pub fn chain_position(&self) -> Option<(u8, u8)> {
        let state = self.read_state();
        state.port.zip(state.index)
    }

    /// Firmware version string, taken from the first `GetProductInfo` report.
    #[must_use]
    pub fn firmware(&self) -> Option<String> {
        self.read_state().firmware.clone()
    }

    /// Panel mode and frame counter from the last handshake reply.
    #[must_use]
    pub fn handshake(&self) -> Option<(u8, u16)> {
        let state = self.read_state();
        state.mode.zip(state.frame_index)
    }

    fn read_state(&self) -> std::sync::RwLockReadGuard<'_, TlLcdState> {
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn command(command: TlLcdCommand, payload: &[u8], expects_response: bool) -> ProtocolCommand {
        ProtocolCommand {
            data: build_tl_lcd_packet(command, payload),
            expects_response,
            ..Default::default()
        }
    }

    /// Build an `LcdControl` command from the current settings.
    fn control_command(&self, mode: TlLcdMode) -> ProtocolCommand {
        let state = self.read_state();
        let mut payload = [0_u8; TL_LCD_CONTROL_PAYLOAD_LEN];
        payload[0] = mode as u8;
        payload[4] = state.brightness;
        payload[5] = state.fps;
        payload[6] = rotation_byte(state.rotation);
        drop(state);

        Self::command(TlLcdCommand::LcdControl, &payload, true)
    }

    fn parse_serial_reply(&self, payload: &[u8]) {
        if payload.len() < TL_LCD_SERIAL_LEN + 2 {
            return;
        }

        let serial: Vec<u8> = payload[..TL_LCD_SERIAL_LEN]
            .iter()
            .take_while(|byte| **byte != 0x00)
            .copied()
            .collect();
        let serial = String::from_utf8_lossy(&serial).trim().to_owned();

        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.serial = (!serial.is_empty()).then_some(serial);
        state.port = Some(payload[TL_LCD_SERIAL_LEN]);
        state.index = Some(payload[TL_LCD_SERIAL_LEN + 1]);
    }

    fn parse_handshake_reply(&self, payload: &[u8]) {
        if payload.len() < 3 {
            return;
        }

        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.mode = Some(payload[0]);
        state.frame_index = Some(u16::from_be_bytes([payload[1], payload[2]]));
    }

    fn parse_product_info_reply(&self, payload: &[u8]) {
        let text: Vec<u8> = payload
            .iter()
            .take_while(|byte| **byte != 0x00)
            .copied()
            .collect();
        let text = String::from_utf8_lossy(&text).trim().to_owned();
        if text.is_empty() {
            return;
        }

        // Report order carries the meaning: version first, build date second.
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(existing) = state.firmware.as_deref() {
            debug!(
                firmware = existing,
                discarded = text.as_str(),
                "ignoring trailing product-info report; firmware version already recorded"
            );
        } else {
            state.firmware = Some(text);
        }
    }
}

const fn rotation_byte(rotation: DisplayRotation) -> u8 {
    match rotation {
        DisplayRotation::Deg0 => 0,
        DisplayRotation::Deg90 => 1,
        DisplayRotation::Deg180 => 2,
        DisplayRotation::Deg270 => 3,
    }
}

impl Default for TlLcdProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl Protocol for TlLcdProtocol {
    fn name(&self) -> &'static str {
        "Lian Li Uni Fan TL LCD"
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        // A session starts by forgetting what the last one learned, so a
        // panel reflashed between connects reports its new firmware instead
        // of keeping the old string forever.
        self.state
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .firmware = None;

        vec![
            Self::command(TlLcdCommand::ReadSerial, &[], true)
                .with_response_timeout(TL_LCD_INIT_TIMEOUT),
            Self::command(TlLcdCommand::GetHandshake, &[], true)
                .with_response_timeout(TL_LCD_INIT_TIMEOUT),
            Self::command(TlLcdCommand::GetProductInfo, &[], true)
                .with_response_count(TL_LCD_PRODUCT_INFO_REPORTS)
                .with_response_timeout(TL_LCD_INIT_TIMEOUT),
            self.control_command(TlLcdMode::LcdSetting)
                .with_response_timeout(TL_LCD_INIT_TIMEOUT),
        ]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, _colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        // The panel is a display; it carries no addressable LEDs.
        Vec::new()
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
        if let Err(error) = encode_chunked_display_frame(&self.sync_layout, jpeg_data, commands) {
            // Skip-and-warn: the display seam has no error channel, and a
            // frame the counter cannot address must not go out truncated.
            warn!(%error, jpeg_bytes = jpeg_data.len(), "skipping TL LCD display frame");
        }

        Some(())
    }

    fn encode_display_payload_into(
        &self,
        payload: DisplayFramePayload<'_>,
        commands: &mut Vec<ProtocolCommand>,
    ) -> Option<()> {
        match payload.format {
            DisplayFrameFormat::Jpeg => self.encode_display_frame_into(payload.data, commands),
            DisplayFrameFormat::Rgb => None,
        }
    }

    fn encode_display_setting(&self, setting: DisplaySetting) -> Option<Vec<ProtocolCommand>> {
        {
            let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
            match setting {
                DisplaySetting::Brightness(brightness) => {
                    state.brightness = brightness.min(TL_LCD_DEFAULT_BRIGHTNESS);
                }
                DisplaySetting::Rotation(rotation) => state.rotation = rotation,
                // The panel tops out at 30fps; a higher request would be a
                // number the hardware cannot honour, and zero would stall it.
                DisplaySetting::FrameRate(fps) => {
                    state.fps = fps.clamp(1, TL_LCD_DEFAULT_FPS);
                }
            }
        }

        Some(vec![self.control_command(TlLcdMode::LcdSetting)])
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        if data.len() < TL_LCD_HEADER_LEN {
            return Err(ProtocolError::MalformedResponse {
                detail: format!(
                    "panel reply of {} bytes is shorter than the {TL_LCD_HEADER_LEN}-byte header",
                    data.len()
                ),
            });
        }

        let command = data[1];
        let declared = usize::from(u16::from_be_bytes([data[9], data[10]]));
        // Replies arrive as short reports, so the declared length can exceed
        // what the transport handed back; trust the bytes in hand.
        let available = data.len() - TL_LCD_HEADER_LEN;
        let payload = &data[TL_LCD_HEADER_LEN..TL_LCD_HEADER_LEN + declared.min(available)];

        match command {
            command if command == TlLcdCommand::ReadSerial as u8 => {
                self.parse_serial_reply(payload);
            }
            command if command == TlLcdCommand::GetHandshake as u8 => {
                self.parse_handshake_reply(payload);
            }
            command if command == TlLcdCommand::GetProductInfo as u8 => {
                self.parse_product_info_reply(payload);
            }
            _ => {}
        }

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: payload.to_vec(),
        })
    }

    fn response_timeout(&self) -> Duration {
        TL_LCD_STEADY_TIMEOUT
    }

    fn zones(&self) -> Vec<SegmentInfo> {
        vec![SegmentInfo {
            name: "Display".to_owned(),
            led_count: 0,
            topology: DeviceTopologyHint::Display {
                width: TL_LCD_RESOLUTION,
                height: TL_LCD_RESOLUTION,
                circular: true,
            },
            color_format: DeviceColorFormat::Jpeg,
            layout_hint: None,
        }]
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: 0,
            supports_direct: false,
            supports_brightness: false,
            has_display: true,
            display_resolution: Some((TL_LCD_RESOLUTION, TL_LCD_RESOLUTION)),
            max_fps: TL_LCD_MAX_FPS,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        0
    }

    fn frame_interval(&self) -> Duration {
        TL_LCD_FRAME_INTERVAL
    }
}
