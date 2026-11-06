//! LIFX LAN wire format: the 36-byte header, the messages this driver
//! sends, and decoders for the State messages it reads (Spec 88 §2 to §6).
//!
//! Every multi-byte field is little-endian. Reserved fields are written as
//! zero and ignored on receive, because real devices put data in them.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::color::Hsbk;

/// UDP port every LIFX device listens on.
pub const DEFAULT_PORT: u16 = 56_700;
/// Length of the frame header, frame address, and protocol header.
pub const HEADER_LEN: usize = 36;
/// Protocol number carried in the low 12 bits of the frame flags.
pub const PROTOCOL_NUMBER: u16 = 1024;
/// Colors carried by one SetExtendedColorZones or StateExtendedColorZones.
pub const EXTENDED_ZONES_PER_MESSAGE: usize = 82;
/// Colors carried by one Set64.
pub const SET64_COLORS: usize = 64;
/// Tile records in one StateDeviceChain.
pub const DEVICE_CHAIN_RECORDS: usize = 16;

const PROTOCOL_MASK: u16 = 0x0FFF;
const ADDRESSABLE_BIT: u16 = 1 << 12;
const TAGGED_BIT: u16 = 1 << 13;
const RES_REQUIRED_BIT: u8 = 0x01;
const ACK_REQUIRED_BIT: u8 = 0x02;
const LABEL_LEN: usize = 32;
const TILE_RECORD_LEN: usize = 55;
const HSBK_LEN: usize = 8;

/// LIFX message type numbers used by this driver.
pub mod message_type {
    pub const GET_SERVICE: u16 = 2;
    pub const STATE_SERVICE: u16 = 3;
    pub const GET_HOST_FIRMWARE: u16 = 14;
    pub const STATE_HOST_FIRMWARE: u16 = 15;
    pub const GET_LABEL: u16 = 23;
    pub const STATE_LABEL: u16 = 25;
    pub const GET_VERSION: u16 = 32;
    pub const STATE_VERSION: u16 = 33;
    pub const ACKNOWLEDGEMENT: u16 = 45;
    pub const SET_COLOR: u16 = 102;
    pub const SET_LIGHT_POWER: u16 = 117;
    pub const STATE_UNHANDLED: u16 = 223;
    pub const SET_COLOR_ZONES: u16 = 501;
    pub const GET_COLOR_ZONES: u16 = 502;
    pub const STATE_ZONE: u16 = 503;
    pub const STATE_MULTI_ZONE: u16 = 506;
    pub const SET_EXTENDED_COLOR_ZONES: u16 = 510;
    pub const GET_EXTENDED_COLOR_ZONES: u16 = 511;
    pub const STATE_EXTENDED_COLOR_ZONES: u16 = 512;
    pub const GET_DEVICE_CHAIN: u16 = 701;
    pub const STATE_DEVICE_CHAIN: u16 = 702;
    pub const SET64: u16 = 715;
    pub const COPY_FRAME_BUFFER: u16 = 716;
}

/// Errors raised while encoding or decoding LIFX packets.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProtocolError {
    #[error("LIFX packet is {actual} bytes, shorter than the {expected}-byte minimum")]
    Truncated { expected: usize, actual: usize },
    #[error("LIFX packet declares {declared} bytes but carries {actual}")]
    SizeMismatch { declared: usize, actual: usize },
    #[error("LIFX packet uses protocol {0}, expected 1024")]
    UnsupportedProtocol(u16),
    #[error("LIFX message type {actual} does not match expected type {expected}")]
    UnexpectedType { expected: u16, actual: u16 },
    #[error("LIFX message carries {actual} colors, more than the {max} it can hold")]
    TooManyColors { max: usize, actual: usize },
    #[error("LIFX device chain record {index} reports an empty {width}x{height} tile")]
    EmptyTile { index: usize, width: u8, height: u8 },
    #[error("invalid LIFX serial {0:?}")]
    InvalidSerial(String),
}

/// Six-byte device serial, which is also the device MAC address.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Serial(pub [u8; 6]);

impl Serial {
    /// The all-zero target that addresses every device.
    pub const BROADCAST: Self = Self([0; 6]);

    /// Whether this is the all-zero broadcast target.
    #[must_use]
    pub fn is_broadcast(self) -> bool {
        self == Self::BROADCAST
    }

    /// The eight-byte header target: the serial left aligned, two zero bytes.
    #[must_use]
    pub fn to_target(self) -> [u8; 8] {
        let mut target = [0_u8; 8];
        target[..6].copy_from_slice(&self.0);
        target
    }

    /// Read a serial from the first six bytes of a header target.
    #[must_use]
    pub fn from_target(target: [u8; 8]) -> Self {
        let mut serial = [0_u8; 6];
        serial.copy_from_slice(&target[..6]);
        Self(serial)
    }
}

impl fmt::Display for Serial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Serial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Serial({self})")
    }
}

impl FromStr for Serial {
    type Err = ProtocolError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let hex: String = raw
            .chars()
            .filter(|ch| !matches!(ch, ':' | '-' | '.'))
            .collect();
        if hex.len() != 12 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(ProtocolError::InvalidSerial(raw.to_owned()));
        }
        let mut serial = [0_u8; 6];
        for (index, byte) in serial.iter_mut().enumerate() {
            let pair = &hex[index * 2..index * 2 + 2];
            *byte = u8::from_str_radix(pair, 16)
                .map_err(|_| ProtocolError::InvalidSerial(raw.to_owned()))?;
        }
        Ok(Self(serial))
    }
}

/// Addressing and flags for one outgoing message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameAddress {
    /// Client identifier echoed in replies. Never 0 or 1.
    pub source: u32,
    /// Destination device, or [`Serial::BROADCAST`] for every device.
    pub target: Serial,
    /// Ask the device for an Acknowledgement (45).
    pub ack_required: bool,
    /// Ask the device for a State reply to a Set message.
    pub res_required: bool,
    /// Wrapping message sequence number.
    pub sequence: u8,
}

/// Multizone application request carried by SetColorZones and
/// SetExtendedColorZones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ApplyMode {
    /// Buffer the change on the device without showing it.
    NoApply = 0,
    /// Show this change and everything buffered before it.
    Apply = 1,
    /// Show the buffer and ignore this message's colors.
    ApplyOnly = 2,
}

impl From<ApplyMode> for u8 {
    fn from(mode: ApplyMode) -> Self {
        match mode {
            ApplyMode::NoApply => 0,
            ApplyMode::Apply => 1,
            ApplyMode::ApplyOnly => 2,
        }
    }
}

/// One message this driver sends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Message<'a> {
    /// GetService (2): discovery.
    GetService,
    /// GetHostFirmware (14).
    GetHostFirmware,
    /// GetLabel (23).
    GetLabel,
    /// GetVersion (32).
    GetVersion,
    /// SetColor (102): paint every zone one color.
    SetColor { color: Hsbk, duration_ms: u32 },
    /// SetLightPower (117).
    SetLightPower { level: u16, duration_ms: u32 },
    /// SetColorZones (501): paint an inclusive zone range.
    SetColorZones {
        start_index: u8,
        end_index: u8,
        color: Hsbk,
        duration_ms: u32,
        apply: ApplyMode,
    },
    /// GetColorZones (502).
    GetColorZones { start_index: u8, end_index: u8 },
    /// SetExtendedColorZones (510): up to 82 zones starting at `zone_index`.
    SetExtendedColorZones {
        duration_ms: u32,
        apply: ApplyMode,
        zone_index: u16,
        colors: &'a [Hsbk],
    },
    /// GetExtendedColorZones (511).
    GetExtendedColorZones,
    /// GetDeviceChain (701).
    GetDeviceChain,
    /// Set64 (715): up to 64 colors into one tile's frame buffer.
    Set64 {
        tile_index: u8,
        length: u8,
        fb_index: u8,
        x: u8,
        y: u8,
        width: u8,
        duration_ms: u32,
        colors: &'a [Hsbk],
    },
    /// CopyFrameBuffer (716).
    CopyFrameBuffer {
        tile_index: u8,
        length: u8,
        src_fb_index: u8,
        dst_fb_index: u8,
        src_x: u8,
        src_y: u8,
        dst_x: u8,
        dst_y: u8,
        width: u8,
        height: u8,
        duration_ms: u32,
    },
}

impl Message<'_> {
    /// Wire message type number.
    #[must_use]
    pub const fn message_type(&self) -> u16 {
        match self {
            Self::GetService => message_type::GET_SERVICE,
            Self::GetHostFirmware => message_type::GET_HOST_FIRMWARE,
            Self::GetLabel => message_type::GET_LABEL,
            Self::GetVersion => message_type::GET_VERSION,
            Self::SetColor { .. } => message_type::SET_COLOR,
            Self::SetLightPower { .. } => message_type::SET_LIGHT_POWER,
            Self::SetColorZones { .. } => message_type::SET_COLOR_ZONES,
            Self::GetColorZones { .. } => message_type::GET_COLOR_ZONES,
            Self::SetExtendedColorZones { .. } => message_type::SET_EXTENDED_COLOR_ZONES,
            Self::GetExtendedColorZones => message_type::GET_EXTENDED_COLOR_ZONES,
            Self::GetDeviceChain => message_type::GET_DEVICE_CHAIN,
            Self::Set64 { .. } => message_type::SET64,
            Self::CopyFrameBuffer { .. } => message_type::COPY_FRAME_BUFFER,
        }
    }

    /// Payload length in bytes.
    #[must_use]
    pub const fn payload_len(&self) -> usize {
        match self {
            Self::GetService
            | Self::GetHostFirmware
            | Self::GetLabel
            | Self::GetVersion
            | Self::GetExtendedColorZones
            | Self::GetDeviceChain => 0,
            Self::SetColor { .. } => 13,
            Self::SetLightPower { .. } => 6,
            Self::SetColorZones { .. } | Self::CopyFrameBuffer { .. } => 15,
            Self::GetColorZones { .. } => 2,
            Self::SetExtendedColorZones { .. } => 8 + EXTENDED_ZONES_PER_MESSAGE * HSBK_LEN,
            Self::Set64 { .. } => 10 + SET64_COLORS * HSBK_LEN,
        }
    }

    fn encode_payload(&self, buf: &mut Vec<u8>) -> Result<(), ProtocolError> {
        match *self {
            Self::GetService
            | Self::GetHostFirmware
            | Self::GetLabel
            | Self::GetVersion
            | Self::GetExtendedColorZones
            | Self::GetDeviceChain => {}
            Self::SetColor { color, duration_ms } => {
                buf.push(0);
                push_hsbk(buf, color);
                buf.extend_from_slice(&duration_ms.to_le_bytes());
            }
            Self::SetLightPower { level, duration_ms } => {
                buf.extend_from_slice(&level.to_le_bytes());
                buf.extend_from_slice(&duration_ms.to_le_bytes());
            }
            Self::SetColorZones {
                start_index,
                end_index,
                color,
                duration_ms,
                apply,
            } => {
                buf.push(start_index);
                buf.push(end_index);
                push_hsbk(buf, color);
                buf.extend_from_slice(&duration_ms.to_le_bytes());
                buf.push(u8::from(apply));
            }
            Self::GetColorZones {
                start_index,
                end_index,
            } => {
                buf.push(start_index);
                buf.push(end_index);
            }
            Self::SetExtendedColorZones {
                duration_ms,
                apply,
                zone_index,
                colors,
            } => {
                let count = color_count(colors, EXTENDED_ZONES_PER_MESSAGE)?;
                buf.extend_from_slice(&duration_ms.to_le_bytes());
                buf.push(u8::from(apply));
                buf.extend_from_slice(&zone_index.to_le_bytes());
                buf.push(count);
                push_padded_colors(buf, colors, EXTENDED_ZONES_PER_MESSAGE);
            }
            Self::Set64 {
                tile_index,
                length,
                fb_index,
                x,
                y,
                width,
                duration_ms,
                colors,
            } => {
                color_count(colors, SET64_COLORS)?;
                buf.extend_from_slice(&[tile_index, length, fb_index, x, y, width]);
                buf.extend_from_slice(&duration_ms.to_le_bytes());
                push_padded_colors(buf, colors, SET64_COLORS);
            }
            Self::CopyFrameBuffer {
                tile_index,
                length,
                src_fb_index,
                dst_fb_index,
                src_x,
                src_y,
                dst_x,
                dst_y,
                width,
                height,
                duration_ms,
            } => {
                buf.extend_from_slice(&[
                    tile_index,
                    length,
                    src_fb_index,
                    dst_fb_index,
                    src_x,
                    src_y,
                    dst_x,
                    dst_y,
                    width,
                    height,
                ]);
                buf.extend_from_slice(&duration_ms.to_le_bytes());
                buf.push(0);
            }
        }
        Ok(())
    }
}

/// Append one encoded message (header and payload) to `buf`.
///
/// The target being [`Serial::BROADCAST`] sets the tagged bit.
///
/// # Errors
///
/// Returns [`ProtocolError::TooManyColors`] when a color slice exceeds what
/// the message can carry.
pub fn encode_message(
    buf: &mut Vec<u8>,
    address: &FrameAddress,
    message: &Message<'_>,
) -> Result<(), ProtocolError> {
    let start = buf.len();
    let size = HEADER_LEN + message.payload_len();
    let size_field = u16::try_from(size).map_err(|_| ProtocolError::TooManyColors {
        max: usize::from(u16::MAX),
        actual: size,
    })?;
    buf.reserve(size);

    let mut flags = PROTOCOL_NUMBER | ADDRESSABLE_BIT;
    if address.target.is_broadcast() {
        flags |= TAGGED_BIT;
    }
    buf.extend_from_slice(&size_field.to_le_bytes());
    buf.extend_from_slice(&flags.to_le_bytes());
    buf.extend_from_slice(&address.source.to_le_bytes());

    buf.extend_from_slice(&address.target.to_target());
    buf.extend_from_slice(&[0; 6]);
    let mut address_flags = 0_u8;
    if address.res_required {
        address_flags |= RES_REQUIRED_BIT;
    }
    if address.ack_required {
        address_flags |= ACK_REQUIRED_BIT;
    }
    buf.push(address_flags);
    buf.push(address.sequence);

    buf.extend_from_slice(&[0; 8]);
    buf.extend_from_slice(&message.message_type().to_le_bytes());
    buf.extend_from_slice(&[0; 2]);

    if let Err(error) = message.encode_payload(buf) {
        buf.truncate(start);
        return Err(error);
    }
    debug_assert_eq!(
        buf.len() - start,
        size,
        "encoded LIFX size must match header"
    );
    Ok(())
}

/// Encode one message into a fresh buffer.
///
/// # Errors
///
/// See [`encode_message`].
pub fn encode_to_vec(
    address: &FrameAddress,
    message: &Message<'_>,
) -> Result<Vec<u8>, ProtocolError> {
    let mut buf = Vec::with_capacity(HEADER_LEN + message.payload_len());
    encode_message(&mut buf, address, message)?;
    Ok(buf)
}

/// Decoded header of a received packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub size: u16,
    pub tagged: bool,
    pub source: u32,
    pub target: Serial,
    pub res_required: bool,
    pub ack_required: bool,
    pub sequence: u8,
    pub message_type: u16,
}

/// A received packet: its header and the payload bytes after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet<'a> {
    pub header: Header,
    pub payload: &'a [u8],
}

/// Decode the header of a received datagram.
///
/// Reserved fields are ignored. Trailing bytes beyond the declared size are
/// ignored as well.
///
/// # Errors
///
/// Returns an error when the datagram is shorter than a header or than its
/// declared size, or when the protocol number is not 1024.
pub fn decode_packet(bytes: &[u8]) -> Result<Packet<'_>, ProtocolError> {
    if bytes.len() < HEADER_LEN {
        return Err(ProtocolError::Truncated {
            expected: HEADER_LEN,
            actual: bytes.len(),
        });
    }
    let size = read_u16(bytes, 0);
    let declared = usize::from(size);
    if declared < HEADER_LEN || declared > bytes.len() {
        return Err(ProtocolError::SizeMismatch {
            declared,
            actual: bytes.len(),
        });
    }
    let flags = read_u16(bytes, 2);
    let protocol = flags & PROTOCOL_MASK;
    if protocol != PROTOCOL_NUMBER {
        return Err(ProtocolError::UnsupportedProtocol(protocol));
    }
    let mut target = [0_u8; 8];
    target.copy_from_slice(&bytes[8..16]);
    let address_flags = bytes[22];

    Ok(Packet {
        header: Header {
            size,
            tagged: flags & TAGGED_BIT != 0,
            source: read_u32(bytes, 4),
            target: Serial::from_target(target),
            res_required: address_flags & RES_REQUIRED_BIT != 0,
            ack_required: address_flags & ACK_REQUIRED_BIT != 0,
            sequence: bytes[23],
            message_type: read_u16(bytes, 32),
        },
        payload: &bytes[HEADER_LEN..declared],
    })
}

/// StateService (3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateService {
    /// 1 is UDP; other values are reserved.
    pub service: u8,
    pub port: u32,
}

impl StateService {
    /// Service value for UDP.
    pub const UDP: u8 = 1;
}

/// StateHostFirmware (15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateHostFirmware {
    pub build: u64,
    pub version_minor: u16,
    pub version_major: u16,
}

impl StateHostFirmware {
    /// Firmware version as the comparable `(major, minor)` tuple.
    #[must_use]
    pub const fn version(&self) -> FirmwareVersion {
        FirmwareVersion {
            major: self.version_major,
            minor: self.version_minor,
        }
    }
}

/// Firmware version compared as `(major, minor)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FirmwareVersion {
    pub major: u16,
    pub minor: u16,
}

impl FirmwareVersion {
    #[must_use]
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }
}

impl fmt::Display for FirmwareVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// StateVersion (33).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateVersion {
    pub vendor: u32,
    pub product: u32,
}

/// StateZone (503).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateZone {
    pub zones_count: u8,
    pub zone_index: u8,
    pub color: Hsbk,
}

/// StateMultiZone (506).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateMultiZone {
    pub zones_count: u8,
    pub zone_index: u8,
    pub colors: [Hsbk; 8],
}

/// StateExtendedColorZones (512).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateExtendedColorZones {
    pub zones_count: u16,
    pub zone_index: u16,
    /// The meaningful colors, `colors_count` entries at most 82.
    pub colors: Vec<Hsbk>,
}

/// One tile record from StateDeviceChain (702).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileRecord {
    pub accel_meas_x: i16,
    pub accel_meas_y: i16,
    pub accel_meas_z: i16,
    pub user_x: f32,
    pub user_y: f32,
    pub width: u8,
    pub height: u8,
    pub supported_frame_buffers: u8,
    pub device_version_vendor: u32,
    pub device_version_product: u32,
    pub firmware_build: u64,
    pub firmware_version: FirmwareVersion,
}

/// StateDeviceChain (702).
#[derive(Debug, Clone, PartialEq)]
pub struct StateDeviceChain {
    pub start_index: u8,
    /// The `tile_devices_count` real records, in chain order.
    pub tiles: Vec<TileRecord>,
}

/// A decoded State message.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Service(StateService),
    HostFirmware(StateHostFirmware),
    Label(String),
    Version(StateVersion),
    Acknowledgement,
    /// StateUnhandled (223): the device does not implement `0`.
    Unhandled(u16),
    Zone(StateZone),
    MultiZone(StateMultiZone),
    ExtendedColorZones(StateExtendedColorZones),
    DeviceChain(StateDeviceChain),
    /// Any other message type, left undecoded.
    Other(u16),
}

/// Decode a received packet's payload according to its message type.
///
/// # Errors
///
/// Returns [`ProtocolError::Truncated`] when the payload is shorter than the
/// message's documented layout, or [`ProtocolError::EmptyTile`] for a chain
/// record with zero width or height.
pub fn decode_state(packet: &Packet<'_>) -> Result<State, ProtocolError> {
    let payload = packet.payload;
    let state = match packet.header.message_type {
        message_type::STATE_SERVICE => {
            require(payload, 5)?;
            State::Service(StateService {
                service: payload[0],
                port: read_u32(payload, 1),
            })
        }
        message_type::STATE_HOST_FIRMWARE => {
            require(payload, 20)?;
            State::HostFirmware(StateHostFirmware {
                build: read_u64(payload, 0),
                version_minor: read_u16(payload, 16),
                version_major: read_u16(payload, 18),
            })
        }
        message_type::STATE_LABEL => {
            require(payload, LABEL_LEN)?;
            State::Label(decode_label(&payload[..LABEL_LEN]))
        }
        message_type::STATE_VERSION => {
            require(payload, 12)?;
            State::Version(StateVersion {
                vendor: read_u32(payload, 0),
                product: read_u32(payload, 4),
            })
        }
        message_type::ACKNOWLEDGEMENT => State::Acknowledgement,
        message_type::STATE_UNHANDLED => {
            require(payload, 2)?;
            State::Unhandled(read_u16(payload, 0))
        }
        message_type::STATE_ZONE => {
            require(payload, 2 + HSBK_LEN)?;
            State::Zone(StateZone {
                zones_count: payload[0],
                zone_index: payload[1],
                color: read_hsbk(payload, 2),
            })
        }
        message_type::STATE_MULTI_ZONE => {
            require(payload, 2 + 8 * HSBK_LEN)?;
            let mut colors = [Hsbk::default(); 8];
            for (index, color) in colors.iter_mut().enumerate() {
                *color = read_hsbk(payload, 2 + index * HSBK_LEN);
            }
            State::MultiZone(StateMultiZone {
                zones_count: payload[0],
                zone_index: payload[1],
                colors,
            })
        }
        message_type::STATE_EXTENDED_COLOR_ZONES => {
            require(payload, 5 + EXTENDED_ZONES_PER_MESSAGE * HSBK_LEN)?;
            let count = usize::from(payload[4]).min(EXTENDED_ZONES_PER_MESSAGE);
            let colors = (0..count)
                .map(|index| read_hsbk(payload, 5 + index * HSBK_LEN))
                .collect();
            State::ExtendedColorZones(StateExtendedColorZones {
                zones_count: read_u16(payload, 0),
                zone_index: read_u16(payload, 2),
                colors,
            })
        }
        message_type::STATE_DEVICE_CHAIN => State::DeviceChain(decode_device_chain(payload)?),
        other => State::Other(other),
    };
    Ok(state)
}

fn decode_device_chain(payload: &[u8]) -> Result<StateDeviceChain, ProtocolError> {
    let len = 1 + DEVICE_CHAIN_RECORDS * TILE_RECORD_LEN + 1;
    require(payload, len)?;
    let count = usize::from(payload[len - 1]).min(DEVICE_CHAIN_RECORDS);
    let mut tiles = Vec::with_capacity(count);
    for index in 0..count {
        let offset = 1 + index * TILE_RECORD_LEN;
        let record = &payload[offset..offset + TILE_RECORD_LEN];
        let tile = TileRecord {
            accel_meas_x: read_i16(record, 0),
            accel_meas_y: read_i16(record, 2),
            accel_meas_z: read_i16(record, 4),
            user_x: read_f32(record, 8),
            user_y: read_f32(record, 12),
            width: record[16],
            height: record[17],
            supported_frame_buffers: record[18],
            device_version_vendor: read_u32(record, 19),
            device_version_product: read_u32(record, 23),
            firmware_build: read_u64(record, 31),
            firmware_version: FirmwareVersion {
                major: read_u16(record, 49),
                minor: read_u16(record, 47),
            },
        };
        if tile.width == 0 || tile.height == 0 {
            return Err(ProtocolError::EmptyTile {
                index,
                width: tile.width,
                height: tile.height,
            });
        }
        tiles.push(tile);
    }
    Ok(StateDeviceChain {
        start_index: payload[0],
        tiles,
    })
}

fn decode_label(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
}

fn color_count(colors: &[Hsbk], max: usize) -> Result<u8, ProtocolError> {
    if colors.len() > max {
        return Err(ProtocolError::TooManyColors {
            max,
            actual: colors.len(),
        });
    }
    u8::try_from(colors.len()).map_err(|_| ProtocolError::TooManyColors {
        max,
        actual: colors.len(),
    })
}

fn push_hsbk(buf: &mut Vec<u8>, color: Hsbk) {
    buf.extend_from_slice(&color.to_le_bytes());
}

fn push_padded_colors(buf: &mut Vec<u8>, colors: &[Hsbk], slots: usize) {
    for color in colors {
        push_hsbk(buf, *color);
    }
    let padding = slots.saturating_sub(colors.len()) * HSBK_LEN;
    buf.resize(buf.len() + padding, 0);
}

fn require(payload: &[u8], expected: usize) -> Result<(), ProtocolError> {
    if payload.len() < expected {
        return Err(ProtocolError::Truncated {
            expected,
            actual: payload.len(),
        });
    }
    Ok(())
}

fn read_hsbk(bytes: &[u8], offset: usize) -> Hsbk {
    let mut raw = [0_u8; HSBK_LEN];
    raw.copy_from_slice(&bytes[offset..offset + HSBK_LEN]);
    Hsbk::from_le_bytes(raw)
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_i16(bytes: &[u8], offset: usize) -> i16 {
    i16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    let mut raw = [0_u8; 4];
    raw.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(raw)
}

fn read_f32(bytes: &[u8], offset: usize) -> f32 {
    let mut raw = [0_u8; 4];
    raw.copy_from_slice(&bytes[offset..offset + 4]);
    f32::from_le_bytes(raw)
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut raw = [0_u8; 8];
    raw.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(raw)
}
