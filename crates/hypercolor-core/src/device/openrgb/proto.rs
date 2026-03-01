//! `OpenRGB` SDK binary wire protocol.
//!
//! Implements serialization and deserialization for the `OpenRGB` SDK
//! binary protocol (TCP port 6742). All multi-byte integers are
//! little-endian. Strings use the "bstring" format: `u16 length`
//! (including null terminator) + UTF-8 bytes + `0x00`.
//!
//! Reference: `OpenRGB` SDK protocol documentation (spec 05, section 3).

use std::io::Cursor;

use anyhow::{Context, Result, bail, ensure};

// ── Constants ────────────────────────────────────────────────────────────

/// Magic bytes at the start of every `OpenRGB` SDK packet.
pub const MAGIC: [u8; 4] = [b'O', b'R', b'G', b'B'];

/// Header size in bytes: magic (4) + device index (4) + command (4) + length (4).
pub const HEADER_SIZE: usize = 16;

/// Default `OpenRGB` SDK server port.
pub const DEFAULT_PORT: u16 = 6742;

/// Default `OpenRGB` SDK server host.
pub const DEFAULT_HOST: &str = "127.0.0.1";

/// Client name sent during handshake.
pub const CLIENT_NAME: &str = "Hypercolor";

/// Maximum protocol version we support.
pub const MAX_PROTOCOL_VERSION: u32 = 4;

// ── Packet Commands ──────────────────────────────────────────────────────

/// `OpenRGB` SDK command identifiers.
///
/// Only commands used by the Hypercolor bridge are included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Command {
    /// Request the number of controllers. Response: `u32` count.
    RequestControllerCount = 0,

    /// Request full data for a controller. Payload: protocol version `u32`.
    RequestControllerData = 1,

    /// Negotiate protocol version. Payload: client's max version `u32`.
    RequestProtocolVersion = 40,

    /// Register a client name. Payload: null-terminated string (no length prefix).
    SetClientName = 50,

    /// Server notification that the device list changed. No payload.
    DeviceListUpdated = 100,

    /// Resize a zone. Payload: zone index `u32` + new size `u32`.
    ResizeZone = 1000,

    /// Update all LEDs on a controller. Payload: data size `u32` + count `u16` + colors.
    UpdateLeds = 1050,

    /// Update LEDs in a single zone. Payload: data size `u32` + zone `u32` + count `u16` + colors.
    UpdateZoneLeds = 1051,

    /// Update a single LED. Payload: LED index `u32` + color (4 bytes).
    UpdateSingleLed = 1052,

    /// Switch controller to Direct/Custom mode. No payload.
    SetCustomMode = 1100,
}

impl Command {
    /// Convert a raw `u32` command ID to a [`Command`], if recognized.
    pub fn from_u32(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::RequestControllerCount),
            1 => Some(Self::RequestControllerData),
            40 => Some(Self::RequestProtocolVersion),
            50 => Some(Self::SetClientName),
            100 => Some(Self::DeviceListUpdated),
            1000 => Some(Self::ResizeZone),
            1050 => Some(Self::UpdateLeds),
            1051 => Some(Self::UpdateZoneLeds),
            1052 => Some(Self::UpdateSingleLed),
            1100 => Some(Self::SetCustomMode),
            _ => None,
        }
    }

    /// Return the `u32` discriminant for this command.
    #[must_use]
    #[allow(clippy::as_conversions)]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

// ── Packet Header ────────────────────────────────────────────────────────

/// A 16-byte `OpenRGB` SDK packet header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketHeader {
    /// Which controller this packet targets (0-based).
    pub device_index: u32,

    /// The command identifier.
    pub command: u32,

    /// Byte length of the payload following the header.
    pub data_length: u32,
}

impl PacketHeader {
    /// Serialize the header into a 16-byte buffer.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..4].copy_from_slice(&MAGIC);
        buf[4..8].copy_from_slice(&self.device_index.to_le_bytes());
        buf[8..12].copy_from_slice(&self.command.to_le_bytes());
        buf[12..16].copy_from_slice(&self.data_length.to_le_bytes());
        buf
    }

    /// Parse a 16-byte buffer into a packet header.
    ///
    /// # Errors
    ///
    /// Returns an error if the magic bytes are incorrect.
    pub fn from_bytes(buf: &[u8; HEADER_SIZE]) -> Result<Self> {
        ensure!(
            buf[0..4] == MAGIC,
            "invalid magic bytes: expected ORGB, got {:?}",
            &buf[0..4]
        );

        let device_index = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let command = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let data_length = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);

        Ok(Self {
            device_index,
            command,
            data_length,
        })
    }
}

// ── OpenRGB Zone Types ───────────────────────────────────────────────────

/// `OpenRGB` zone type values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ZoneType {
    /// Single addressable unit.
    Single = 0,
    /// Linear strip of LEDs.
    Linear = 1,
    /// 2D matrix of LEDs.
    Matrix = 2,
}

impl ZoneType {
    /// Parse a `u32` into a zone type, defaulting to [`Single`](ZoneType::Single)
    /// for unrecognized values.
    #[must_use]
    pub fn from_u32(value: u32) -> Self {
        match value {
            1 => Self::Linear,
            2 => Self::Matrix,
            _ => Self::Single,
        }
    }
}

// ── Protocol Data Structures ─────────────────────────────────────────────

/// Parsed zone data from a controller response.
#[derive(Debug, Clone)]
pub struct ZoneData {
    /// Zone name (e.g., "Mainboard", "GPU").
    pub name: String,
    /// Zone type (single, linear, matrix).
    pub zone_type: ZoneType,
    /// Minimum LED count for resizable zones.
    pub leds_min: u32,
    /// Maximum LED count for resizable zones.
    pub leds_max: u32,
    /// Current LED count.
    pub leds_count: u32,
    /// Matrix height (0 if not a matrix).
    pub matrix_height: u32,
    /// Matrix width (0 if not a matrix).
    pub matrix_width: u32,
}

/// Parsed LED data from a controller response.
#[derive(Debug, Clone)]
pub struct LedData {
    /// LED name (e.g., "LED 1", "Key: Escape").
    pub name: String,
    /// Hardware-specific LED value.
    pub value: u32,
}

/// Parsed mode data from a controller response.
#[derive(Debug, Clone)]
pub struct ModeData {
    /// Mode name.
    pub name: String,
    /// Internal mode ID.
    pub value: u32,
    /// Mode capability flags.
    pub flags: u32,
    /// Minimum speed value.
    pub speed_min: u32,
    /// Maximum speed value.
    pub speed_max: u32,
    /// Minimum brightness value (protocol v3+).
    pub brightness_min: u32,
    /// Maximum brightness value (protocol v3+).
    pub brightness_max: u32,
    /// Minimum color count.
    pub colors_min: u32,
    /// Maximum color count.
    pub colors_max: u32,
    /// Current speed.
    pub speed: u32,
    /// Current brightness (protocol v3+).
    pub brightness: u32,
    /// Direction (0=left, 1=right, 2=up, 3=down).
    pub direction: u32,
    /// Color mode (0=none, 1=per-LED, 2=mode-specific, 3=random).
    pub color_mode: u32,
    /// Mode-specific colors (RGBX, 4 bytes each).
    pub colors: Vec<[u8; 4]>,
}

/// An RGB color with padding byte (`OpenRGB` wire format: R, G, B, 0x00).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl RgbColor {
    /// Serialize to 4-byte `OpenRGB` wire format (R, G, B, padding).
    #[must_use]
    pub fn to_wire_bytes(self) -> [u8; 4] {
        [self.r, self.g, self.b, 0]
    }

    /// Parse from 4-byte wire format.
    #[must_use]
    pub fn from_wire_bytes(bytes: [u8; 4]) -> Self {
        Self {
            r: bytes[0],
            g: bytes[1],
            b: bytes[2],
        }
    }
}

/// Complete parsed controller data from a `REQUEST_CONTROLLER_DATA` response.
#[derive(Debug, Clone)]
pub struct ControllerData {
    /// Controller device type (motherboard=0, DRAM=1, GPU=2, etc.).
    pub device_type: u32,
    /// Controller name (e.g., "ASUS Aura LED Controller").
    pub name: String,
    /// Vendor string (protocol v1+).
    pub vendor: String,
    /// Human-readable description.
    pub description: String,
    /// Firmware/driver version string.
    pub version: String,
    /// Serial number.
    pub serial: String,
    /// Location string (e.g., "HID: /dev/hidraw3").
    pub location: String,
    /// Active mode index.
    pub active_mode: u32,
    /// Available modes.
    pub modes: Vec<ModeData>,
    /// Zones within this controller.
    pub zones: Vec<ZoneData>,
    /// Individual LEDs.
    pub leds: Vec<LedData>,
    /// Current LED colors.
    pub colors: Vec<RgbColor>,
}

// ── Wire Serialization ───────────────────────────────────────────────────

/// Convert a cursor position (`u64`) to `usize`, used for buffer indexing.
///
/// # Panics
///
/// Panics if the position exceeds `usize::MAX` (impossible on 64-bit targets,
/// and extremely unlikely on 32-bit targets given protocol packet sizes).
fn cursor_pos(cursor: &Cursor<&[u8]>) -> usize {
    usize::try_from(cursor.position()).expect("cursor position fits in usize")
}

/// Advance a cursor position by `offset` bytes.
fn advance_cursor(cursor: &mut Cursor<&[u8]>, offset: usize) {
    #[allow(clippy::as_conversions)]
    cursor.set_position(cursor.position() + offset as u64);
}

/// Read a little-endian `u16` from a cursor.
fn read_u16(cursor: &mut Cursor<&[u8]>) -> Result<u16> {
    let pos = cursor_pos(cursor);
    let data = cursor.get_ref();
    ensure!(
        pos + 2 <= data.len(),
        "buffer underflow reading u16 at offset {pos}"
    );
    let value = u16::from_le_bytes([data[pos], data[pos + 1]]);
    advance_cursor(cursor, 2);
    Ok(value)
}

/// Read a little-endian `u32` from a cursor.
fn read_u32(cursor: &mut Cursor<&[u8]>) -> Result<u32> {
    let pos = cursor_pos(cursor);
    let data = cursor.get_ref();
    ensure!(
        pos + 4 <= data.len(),
        "buffer underflow reading u32 at offset {pos}"
    );
    let value = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
    advance_cursor(cursor, 4);
    Ok(value)
}

/// Read a bstring (length-prefixed, null-terminated) from a cursor.
///
/// Format: `u16 length` (includes null terminator) + `UTF-8 bytes` + `0x00`.
fn read_bstring(cursor: &mut Cursor<&[u8]>) -> Result<String> {
    let length = read_u16(cursor)?;
    let pos = cursor_pos(cursor);
    let data = cursor.get_ref();

    if length == 0 {
        return Ok(String::new());
    }

    let str_len = usize::from(length);
    ensure!(
        pos + str_len <= data.len(),
        "buffer underflow reading bstring of length {str_len} at offset {pos}"
    );

    // The string includes a null terminator; strip it
    let content_len = if str_len > 0 { str_len - 1 } else { 0 };
    let text = String::from_utf8_lossy(&data[pos..pos + content_len]).into_owned();
    advance_cursor(cursor, str_len);
    Ok(text)
}

/// Write a bstring to a byte buffer.
///
/// Format: `u16 length` (includes null terminator) + `UTF-8 bytes` + `0x00`.
fn write_bstring(buf: &mut Vec<u8>, s: &str) {
    // Length = string bytes + null terminator
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    let length = (s.len() + 1) as u16;
    buf.extend_from_slice(&length.to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
    buf.push(0x00);
}

/// Read a 4-byte `OpenRGB` color (RGBX) from a cursor.
fn read_color(cursor: &mut Cursor<&[u8]>) -> Result<RgbColor> {
    let pos = cursor_pos(cursor);
    let data = cursor.get_ref();
    ensure!(
        pos + 4 <= data.len(),
        "buffer underflow reading color at offset {pos}"
    );
    let color = RgbColor {
        r: data[pos],
        g: data[pos + 1],
        b: data[pos + 2],
    };
    advance_cursor(cursor, 4);
    Ok(color)
}

// ── Packet Construction ──────────────────────────────────────────────────

/// Build a `SET_CLIENT_NAME` packet (command 50).
///
/// The payload is a null-terminated string without a length prefix.
#[must_use]
pub fn build_set_client_name(name: &str) -> Vec<u8> {
    let payload_len = name.len() + 1; // string + null terminator
    let header = PacketHeader {
        device_index: 0,
        command: Command::SetClientName.as_u32(),
        #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
        data_length: payload_len as u32,
    };
    let mut buf = Vec::with_capacity(HEADER_SIZE + payload_len);
    buf.extend_from_slice(&header.to_bytes());
    buf.extend_from_slice(name.as_bytes());
    buf.push(0x00);
    buf
}

/// Build a `REQUEST_PROTOCOL_VERSION` packet (command 40).
#[must_use]
pub fn build_request_protocol_version(version: u32) -> Vec<u8> {
    let header = PacketHeader {
        device_index: 0,
        command: Command::RequestProtocolVersion.as_u32(),
        data_length: 4,
    };
    let mut buf = Vec::with_capacity(HEADER_SIZE + 4);
    buf.extend_from_slice(&header.to_bytes());
    buf.extend_from_slice(&version.to_le_bytes());
    buf
}

/// Build a `REQUEST_CONTROLLER_COUNT` packet (command 0).
#[must_use]
pub fn build_request_controller_count() -> Vec<u8> {
    let header = PacketHeader {
        device_index: 0,
        command: Command::RequestControllerCount.as_u32(),
        data_length: 0,
    };
    header.to_bytes().to_vec()
}

/// Build a `REQUEST_CONTROLLER_DATA` packet (command 1).
#[must_use]
pub fn build_request_controller_data(device_index: u32, protocol_version: u32) -> Vec<u8> {
    let header = PacketHeader {
        device_index,
        command: Command::RequestControllerData.as_u32(),
        data_length: 4,
    };
    let mut buf = Vec::with_capacity(HEADER_SIZE + 4);
    buf.extend_from_slice(&header.to_bytes());
    buf.extend_from_slice(&protocol_version.to_le_bytes());
    buf
}

/// Build a `RGBCONTROLLER_SETCUSTOMMODE` packet (command 1100).
#[must_use]
pub fn build_set_custom_mode(device_index: u32) -> Vec<u8> {
    let header = PacketHeader {
        device_index,
        command: Command::SetCustomMode.as_u32(),
        data_length: 0,
    };
    header.to_bytes().to_vec()
}

/// Build a `RGBCONTROLLER_UPDATELEDS` packet (command 1050).
///
/// Payload format: `data_size (u32)` + `led_count (u16)` + `colors (4 bytes each)`.
/// `data_size` covers everything after itself: `u16` (2 bytes) + `4 * count`.
#[must_use]
#[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
pub fn build_update_leds(device_index: u32, colors: &[[u8; 3]]) -> Vec<u8> {
    let led_count = colors.len() as u16;
    // data_size = sizeof(u16) + 4 * led_count
    let color_bytes = 4 * u32::from(led_count);
    let data_size = 2 + color_bytes;
    let payload_len = 4 + data_size; // data_size field (4) + data_size bytes

    let header = PacketHeader {
        device_index,
        command: Command::UpdateLeds.as_u32(),
        data_length: payload_len,
    };

    let mut buf = Vec::with_capacity(HEADER_SIZE + payload_len as usize);
    buf.extend_from_slice(&header.to_bytes());
    buf.extend_from_slice(&data_size.to_le_bytes());
    buf.extend_from_slice(&led_count.to_le_bytes());

    for &[r, g, b] in colors {
        buf.extend_from_slice(&[r, g, b, 0x00]);
    }

    buf
}

/// Build a `RGBCONTROLLER_UPDATEZONELEDS` packet (command 1051).
///
/// Payload format: `data_size (u32)` + `zone_index (u32)` + `led_count (u16)` + `colors`.
#[must_use]
#[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
pub fn build_update_zone_leds(device_index: u32, zone_index: u32, colors: &[[u8; 3]]) -> Vec<u8> {
    let led_count = colors.len() as u16;
    // data_size = sizeof(u32) zone_index + sizeof(u16) led_count + 4 * led_count
    let color_bytes = 4 * u32::from(led_count);
    let data_size = 4 + 2 + color_bytes;
    let payload_len = 4 + data_size; // data_size field (4) + data_size bytes

    let header = PacketHeader {
        device_index,
        command: Command::UpdateZoneLeds.as_u32(),
        data_length: payload_len,
    };

    let mut buf = Vec::with_capacity(HEADER_SIZE + payload_len as usize);
    buf.extend_from_slice(&header.to_bytes());
    buf.extend_from_slice(&data_size.to_le_bytes());
    buf.extend_from_slice(&zone_index.to_le_bytes());
    buf.extend_from_slice(&led_count.to_le_bytes());

    for &[r, g, b] in colors {
        buf.extend_from_slice(&[r, g, b, 0x00]);
    }

    buf
}

// ── Response Parsing ─────────────────────────────────────────────────────

/// Parse the controller count from a `REQUEST_CONTROLLER_COUNT` response payload.
pub fn parse_controller_count(payload: &[u8]) -> Result<u32> {
    ensure!(
        payload.len() >= 4,
        "controller count payload too short: {} bytes",
        payload.len()
    );
    Ok(u32::from_le_bytes([
        payload[0], payload[1], payload[2], payload[3],
    ]))
}

/// Parse the negotiated protocol version from a `REQUEST_PROTOCOL_VERSION` response payload.
pub fn parse_protocol_version(payload: &[u8]) -> Result<u32> {
    ensure!(
        payload.len() >= 4,
        "protocol version payload too short: {} bytes",
        payload.len()
    );
    Ok(u32::from_le_bytes([
        payload[0], payload[1], payload[2], payload[3],
    ]))
}

/// Parse mode data from a cursor, respecting the negotiated protocol version.
#[allow(clippy::as_conversions)]
fn parse_mode(cursor: &mut Cursor<&[u8]>, protocol_version: u32) -> Result<ModeData> {
    let name = read_bstring(cursor).context("reading mode name")?;
    let value = read_u32(cursor).context("reading mode value")?;
    let flags = read_u32(cursor).context("reading mode flags")?;
    let speed_min = read_u32(cursor).context("reading speed_min")?;
    let speed_max = read_u32(cursor).context("reading speed_max")?;

    let (brightness_min, brightness_max) = if protocol_version >= 3 {
        let bmin = read_u32(cursor).context("reading brightness_min")?;
        let bmax = read_u32(cursor).context("reading brightness_max")?;
        (bmin, bmax)
    } else {
        (0, 0)
    };

    let colors_min = read_u32(cursor).context("reading colors_min")?;
    let colors_max = read_u32(cursor).context("reading colors_max")?;
    let speed = read_u32(cursor).context("reading speed")?;

    let brightness = if protocol_version >= 3 {
        read_u32(cursor).context("reading brightness")?
    } else {
        0
    };

    let direction = read_u32(cursor).context("reading direction")?;
    let color_mode = read_u32(cursor).context("reading color_mode")?;

    let num_colors = read_u16(cursor).context("reading mode num_colors")?;
    let mut colors = Vec::with_capacity(usize::from(num_colors));
    for _ in 0..num_colors {
        let c = read_color(cursor).context("reading mode color")?;
        colors.push([c.r, c.g, c.b, 0]);
    }

    Ok(ModeData {
        name,
        value,
        flags,
        speed_min,
        speed_max,
        brightness_min,
        brightness_max,
        colors_min,
        colors_max,
        speed,
        brightness,
        direction,
        color_mode,
        colors,
    })
}

/// Parse zone data from a cursor.
fn parse_zone(cursor: &mut Cursor<&[u8]>) -> Result<ZoneData> {
    let name = read_bstring(cursor).context("reading zone name")?;
    let zone_type_raw = read_u32(cursor).context("reading zone type")?;
    let zone_type = ZoneType::from_u32(zone_type_raw);
    let leds_min = read_u32(cursor).context("reading leds_min")?;
    let leds_max = read_u32(cursor).context("reading leds_max")?;
    let leds_count = read_u32(cursor).context("reading leds_count")?;

    let matrix_len = read_u16(cursor).context("reading matrix_len")?;

    let (matrix_height, matrix_width) = if matrix_len > 0 {
        let height = read_u32(cursor).context("reading matrix_height")?;
        let width = read_u32(cursor).context("reading matrix_width")?;
        // Skip the matrix data (u32 per cell)
        let cells = height * width;
        let skip_bytes = u64::from(cells) * 4;
        let new_pos = cursor.position() + skip_bytes;
        cursor.set_position(new_pos);
        (height, width)
    } else {
        (0, 0)
    };

    Ok(ZoneData {
        name,
        zone_type,
        leds_min,
        leds_max,
        leds_count,
        matrix_height,
        matrix_width,
    })
}

/// Parse LED data from a cursor.
fn parse_led(cursor: &mut Cursor<&[u8]>) -> Result<LedData> {
    let name = read_bstring(cursor).context("reading LED name")?;
    let value = read_u32(cursor).context("reading LED value")?;
    Ok(LedData { name, value })
}

/// Parse a full `REQUEST_CONTROLLER_DATA` response payload.
///
/// The `protocol_version` determines which fields are present (e.g.,
/// brightness fields exist only in protocol v3+).
#[allow(clippy::as_conversions)]
pub fn parse_controller_data(payload: &[u8], protocol_version: u32) -> Result<ControllerData> {
    let mut cursor = Cursor::new(payload);

    // data_size (u32) — total payload size, already consumed by framing
    let _data_size = read_u32(&mut cursor).context("reading data_size")?;

    let device_type = read_u32(&mut cursor).context("reading device_type")?;
    let name = read_bstring(&mut cursor).context("reading controller name")?;

    let vendor = if protocol_version >= 1 {
        read_bstring(&mut cursor).context("reading vendor")?
    } else {
        String::new()
    };

    let description = read_bstring(&mut cursor).context("reading description")?;
    let version = read_bstring(&mut cursor).context("reading version")?;
    let serial = read_bstring(&mut cursor).context("reading serial")?;
    let location = read_bstring(&mut cursor).context("reading location")?;

    let num_modes = read_u16(&mut cursor).context("reading num_modes")?;
    let active_mode = read_u32(&mut cursor).context("reading active_mode")?;

    let mut modes = Vec::with_capacity(usize::from(num_modes));
    for i in 0..num_modes {
        let mode = parse_mode(&mut cursor, protocol_version)
            .with_context(|| format!("parsing mode {i}"))?;
        modes.push(mode);
    }

    let num_zones = read_u16(&mut cursor).context("reading num_zones")?;
    let mut zones = Vec::with_capacity(usize::from(num_zones));
    for i in 0..num_zones {
        let zone = parse_zone(&mut cursor).with_context(|| format!("parsing zone {i}"))?;
        zones.push(zone);
    }

    let num_leds = read_u16(&mut cursor).context("reading num_leds")?;
    let mut leds = Vec::with_capacity(usize::from(num_leds));
    for i in 0..num_leds {
        let led = parse_led(&mut cursor).with_context(|| format!("parsing LED {i}"))?;
        leds.push(led);
    }

    let num_colors = read_u16(&mut cursor).context("reading num_colors")?;
    let mut colors = Vec::with_capacity(usize::from(num_colors));
    for i in 0..num_colors {
        let color = read_color(&mut cursor).with_context(|| format!("parsing color {i}"))?;
        colors.push(color);
    }

    Ok(ControllerData {
        device_type,
        name,
        vendor,
        description,
        version,
        serial,
        location,
        active_mode,
        modes,
        zones,
        leds,
        colors,
    })
}

// ── Packet Validation ────────────────────────────────────────────────────

/// Validate that a received header matches the expected command.
pub fn validate_response(header: &PacketHeader, expected_command: Command) -> Result<()> {
    let expected = expected_command.as_u32();
    if header.command != expected {
        bail!(
            "unexpected response command: expected {expected}, got {}",
            header.command
        );
    }
    Ok(())
}

// ── Test helpers ─────────────────────────────────────────────────────────

/// Build a minimal mock controller data payload for testing.
///
/// Creates a controller with the given name, vendor, one linear zone, and
/// a set number of LEDs. Uses protocol v1 format (includes vendor string).
#[doc(hidden)]
#[must_use]
pub fn build_mock_controller_payload(
    name: &str,
    vendor: &str,
    zone_name: &str,
    led_count: u16,
) -> Vec<u8> {
    let mut payload = Vec::new();

    // We'll write everything after data_size first, then backfill data_size
    let mut body = Vec::new();

    // device_type: motherboard = 0
    body.extend_from_slice(&0u32.to_le_bytes());
    // name
    write_bstring(&mut body, name);
    // vendor (v1+)
    write_bstring(&mut body, vendor);
    // description
    write_bstring(&mut body, "Test controller");
    // version
    write_bstring(&mut body, "1.0");
    // serial
    write_bstring(&mut body, "SN-001");
    // location
    write_bstring(&mut body, "HID: /dev/hidraw0");

    // num_modes: 1
    body.extend_from_slice(&1u16.to_le_bytes());
    // active_mode: 0
    body.extend_from_slice(&0u32.to_le_bytes());

    // Mode 0: "Direct"
    write_bstring(&mut body, "Direct");
    body.extend_from_slice(&0u32.to_le_bytes()); // value
    body.extend_from_slice(&0u32.to_le_bytes()); // flags
    body.extend_from_slice(&0u32.to_le_bytes()); // speed_min
    body.extend_from_slice(&100u32.to_le_bytes()); // speed_max
    body.extend_from_slice(&0u32.to_le_bytes()); // colors_min
    body.extend_from_slice(&0u32.to_le_bytes()); // colors_max
    body.extend_from_slice(&50u32.to_le_bytes()); // speed
    body.extend_from_slice(&0u32.to_le_bytes()); // direction
    body.extend_from_slice(&1u32.to_le_bytes()); // color_mode (per-LED)
    body.extend_from_slice(&0u16.to_le_bytes()); // num_colors in mode

    // num_zones: 1
    body.extend_from_slice(&1u16.to_le_bytes());

    // Zone 0
    write_bstring(&mut body, zone_name);
    body.extend_from_slice(&1u32.to_le_bytes()); // type: linear
    body.extend_from_slice(&u32::from(led_count).to_le_bytes()); // leds_min
    body.extend_from_slice(&u32::from(led_count).to_le_bytes()); // leds_max
    body.extend_from_slice(&u32::from(led_count).to_le_bytes()); // leds_count
    body.extend_from_slice(&0u16.to_le_bytes()); // matrix_len

    // LEDs
    body.extend_from_slice(&led_count.to_le_bytes());
    for i in 0..led_count {
        write_bstring(&mut body, &format!("LED {i}"));
        body.extend_from_slice(&u32::from(i).to_le_bytes()); // value
    }

    // Colors
    body.extend_from_slice(&led_count.to_le_bytes());
    for _ in 0..led_count {
        body.extend_from_slice(&[0, 0, 0, 0]); // RGBX, all black
    }

    // data_size (u32) prepended
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    let data_size = body.len() as u32;
    payload.extend_from_slice(&data_size.to_le_bytes());
    payload.extend_from_slice(&body);

    payload
}
