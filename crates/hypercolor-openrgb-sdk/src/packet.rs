use crate::error::{OpenRgbError, Result};
use crate::types::{ControllerMode, RgbColor};

/// OpenRGB SDK packet magic.
pub const MAGIC: [u8; 4] = *b"ORGB";

/// OpenRGB SDK packet header size.
pub const HEADER_LEN: usize = 16;

/// Conservative safety cap for a single SDK payload.
pub const MAX_PACKET_PAYLOAD_SIZE: usize = 4 * 1024 * 1024;

/// Oldest negotiated protocol version Hypercolor supports.
pub const MIN_PROTOCOL_VERSION: u32 = 1;

/// Newest protocol version documented for OpenRGB 1.0.
pub const CLIENT_MAX_PROTOCOL_VERSION: u32 = 5;

/// Oldest protocol version that documents device rescan requests.
pub const REQUEST_RESCAN_DEVICES_MIN_PROTOCOL_VERSION: u32 = 5;

/// OpenRGB SDK packet identifiers used by Hypercolor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PacketId {
    RequestControllerCount,
    RequestControllerData,
    RequestProtocolVersion,
    SetClientName,
    DeviceListUpdated,
    RequestRescanDevices,
    ResizeZone,
    UpdateLeds,
    UpdateZoneLeds,
    SetCustomMode,
    UpdateMode,
    SaveMode,
    Unknown(u32),
}

impl PacketId {
    /// Convert a raw SDK packet ID into a typed value.
    #[must_use]
    pub const fn from_raw(value: u32) -> Self {
        match value {
            0 => Self::RequestControllerCount,
            1 => Self::RequestControllerData,
            40 => Self::RequestProtocolVersion,
            50 => Self::SetClientName,
            100 => Self::DeviceListUpdated,
            140 => Self::RequestRescanDevices,
            1000 => Self::ResizeZone,
            1050 => Self::UpdateLeds,
            1051 => Self::UpdateZoneLeds,
            1100 => Self::SetCustomMode,
            1101 => Self::UpdateMode,
            1102 => Self::SaveMode,
            other => Self::Unknown(other),
        }
    }

    /// Raw SDK packet ID.
    #[must_use]
    pub const fn raw(self) -> u32 {
        match self {
            Self::RequestControllerCount => 0,
            Self::RequestControllerData => 1,
            Self::RequestProtocolVersion => 40,
            Self::SetClientName => 50,
            Self::DeviceListUpdated => 100,
            Self::RequestRescanDevices => 140,
            Self::ResizeZone => 1000,
            Self::UpdateLeds => 1050,
            Self::UpdateZoneLeds => 1051,
            Self::SetCustomMode => 1100,
            Self::UpdateMode => 1101,
            Self::SaveMode => 1102,
            Self::Unknown(value) => value,
        }
    }

    /// Whether Hypercolor clients are forbidden from emitting this packet.
    #[must_use]
    pub const fn forbidden_for_client(self) -> bool {
        matches!(self, Self::ResizeZone | Self::SaveMode)
    }
}

/// Decoded SDK packet header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    pub device_index: u32,
    pub packet_id: PacketId,
    pub size: u32,
}

impl PacketHeader {
    /// Decode a packet header from a 16-byte buffer.
    ///
    /// # Errors
    ///
    /// Returns an error when the header is truncated, has bad magic, or
    /// advertises an oversized payload.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_LEN {
            return Err(OpenRgbError::Truncated {
                needed: HEADER_LEN,
                remaining: bytes.len(),
            });
        }

        let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
        if magic != MAGIC {
            return Err(OpenRgbError::InvalidMagic(magic));
        }

        let device_index = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let packet_id = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let size = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        let size_usize = usize::try_from(size).map_err(|_| OpenRgbError::PacketTooLarge {
            size: usize::MAX,
            max: MAX_PACKET_PAYLOAD_SIZE,
        })?;
        if size_usize > MAX_PACKET_PAYLOAD_SIZE {
            return Err(OpenRgbError::PacketTooLarge {
                size: size_usize,
                max: MAX_PACKET_PAYLOAD_SIZE,
            });
        }

        Ok(Self {
            device_index,
            packet_id: PacketId::from_raw(packet_id),
            size,
        })
    }

    /// Encode this header into bytes.
    #[must_use]
    pub fn encode(self) -> [u8; HEADER_LEN] {
        let mut bytes = [0_u8; HEADER_LEN];
        bytes[0..4].copy_from_slice(&MAGIC);
        bytes[4..8].copy_from_slice(&self.device_index.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.packet_id.raw().to_le_bytes());
        bytes[12..16].copy_from_slice(&self.size.to_le_bytes());
        bytes
    }
}

/// Complete SDK packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub header: PacketHeader,
    pub payload: Vec<u8>,
}

impl Packet {
    /// Decode a complete packet from bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the packet is truncated or malformed.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let header = PacketHeader::decode(bytes)?;
        let payload_len =
            usize::try_from(header.size).map_err(|_| OpenRgbError::PacketTooLarge {
                size: usize::MAX,
                max: MAX_PACKET_PAYLOAD_SIZE,
            })?;
        let total = HEADER_LEN
            .checked_add(payload_len)
            .ok_or(OpenRgbError::CountOverflow {
                count: payload_len,
                element_size: 1,
            })?;
        if bytes.len() < total {
            return Err(OpenRgbError::Truncated {
                needed: total,
                remaining: bytes.len(),
            });
        }
        Ok(Self {
            header,
            payload: bytes[HEADER_LEN..total].to_vec(),
        })
    }

    /// Encode this packet into bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_LEN + self.payload.len());
        bytes.extend_from_slice(&self.header.encode());
        bytes.extend_from_slice(&self.payload);
        bytes
    }
}

/// Incremental decoder for TCP streams.
#[derive(Debug, Default)]
pub struct PacketDecoder {
    buffer: Vec<u8>,
}

impl PacketDecoder {
    /// Create an empty stream decoder.
    #[must_use]
    pub const fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Append bytes read from the stream.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Try to decode the next full packet.
    ///
    /// # Errors
    ///
    /// Returns malformed packet errors immediately. Returns `Ok(None)` when the
    /// buffered data is valid but incomplete.
    pub fn next_packet(&mut self) -> Result<Option<Packet>> {
        if self.buffer.len() < HEADER_LEN {
            return Ok(None);
        }

        let header = PacketHeader::decode(&self.buffer[..HEADER_LEN])?;
        let payload_len =
            usize::try_from(header.size).map_err(|_| OpenRgbError::PacketTooLarge {
                size: usize::MAX,
                max: MAX_PACKET_PAYLOAD_SIZE,
            })?;
        let total = HEADER_LEN
            .checked_add(payload_len)
            .ok_or(OpenRgbError::CountOverflow {
                count: payload_len,
                element_size: 1,
            })?;

        if self.buffer.len() < total {
            return Ok(None);
        }

        let payload = self.buffer[HEADER_LEN..total].to_vec();
        self.buffer.drain(..total);
        Ok(Some(Packet { header, payload }))
    }
}

/// Encode a client packet while enforcing Hypercolor's forbidden opcode list.
///
/// # Errors
///
/// Returns an error for forbidden packets or oversized payloads.
pub fn encode_client_packet(
    device_index: u32,
    packet_id: PacketId,
    payload: Vec<u8>,
) -> Result<Vec<u8>> {
    if packet_id.forbidden_for_client() {
        return Err(OpenRgbError::ForbiddenPacket(packet_id));
    }
    if payload.len() > MAX_PACKET_PAYLOAD_SIZE {
        return Err(OpenRgbError::PacketTooLarge {
            size: payload.len(),
            max: MAX_PACKET_PAYLOAD_SIZE,
        });
    }
    let size = u32::try_from(payload.len()).map_err(|_| OpenRgbError::PacketTooLarge {
        size: payload.len(),
        max: MAX_PACKET_PAYLOAD_SIZE,
    })?;
    Ok(Packet {
        header: PacketHeader {
            device_index,
            packet_id,
            size,
        },
        payload,
    }
    .encode())
}

/// Build the payload for `REQUEST_PROTOCOL_VERSION`.
#[must_use]
pub fn request_protocol_version_payload(client_max: u32) -> Vec<u8> {
    client_max.to_le_bytes().to_vec()
}

/// Validate a negotiated protocol version.
///
/// # Errors
///
/// Returns an error when the server version is below or above the supported
/// range.
pub fn validate_protocol_version(version: u32) -> Result<u32> {
    if !(MIN_PROTOCOL_VERSION..=CLIENT_MAX_PROTOCOL_VERSION).contains(&version) {
        return Err(OpenRgbError::UnsupportedProtocolVersion {
            version,
            min: MIN_PROTOCOL_VERSION,
            max: CLIENT_MAX_PROTOCOL_VERSION,
        });
    }
    Ok(version)
}

/// Build the payload for `SET_CLIENT_NAME`.
#[must_use]
pub fn client_name_payload(name: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(name.len() + 1);
    bytes.extend_from_slice(name.as_bytes());
    bytes.push(0);
    bytes
}

/// Build the payload for `REQUEST_CONTROLLER_DATA`.
#[must_use]
pub fn request_controller_data_payload(protocol_version: u32) -> Vec<u8> {
    protocol_version.to_le_bytes().to_vec()
}

/// Build the payload for `UPDATELEDS`.
///
/// # Errors
///
/// Returns an error if the color vector cannot fit in OpenRGB's u16 count.
pub fn update_leds_payload(colors: &[RgbColor]) -> Result<Vec<u8>> {
    let color_count = u16::try_from(colors.len()).map_err(|_| OpenRgbError::CountOverflow {
        count: colors.len(),
        element_size: RgbColor::WIRE_SIZE,
    })?;
    let size = checked_color_block_size(2, colors.len())?;
    let mut payload = Vec::with_capacity(size);
    payload.extend_from_slice(
        &u32::try_from(size)
            .map_err(|_| OpenRgbError::PacketTooLarge {
                size,
                max: MAX_PACKET_PAYLOAD_SIZE,
            })?
            .to_le_bytes(),
    );
    payload.extend_from_slice(&color_count.to_le_bytes());
    for color in colors {
        payload.extend_from_slice(&color.to_wire_bytes());
    }
    Ok(payload)
}

/// Build the payload for `UPDATEZONELEDS`.
///
/// # Errors
///
/// Returns an error if the color vector cannot fit in OpenRGB's u16 count.
pub fn update_zone_leds_payload(zone_index: u32, colors: &[RgbColor]) -> Result<Vec<u8>> {
    let color_count = u16::try_from(colors.len()).map_err(|_| OpenRgbError::CountOverflow {
        count: colors.len(),
        element_size: RgbColor::WIRE_SIZE,
    })?;
    let size = checked_color_block_size(6, colors.len())?;
    let mut payload = Vec::with_capacity(size);
    payload.extend_from_slice(
        &u32::try_from(size)
            .map_err(|_| OpenRgbError::PacketTooLarge {
                size,
                max: MAX_PACKET_PAYLOAD_SIZE,
            })?
            .to_le_bytes(),
    );
    payload.extend_from_slice(&zone_index.to_le_bytes());
    payload.extend_from_slice(&color_count.to_le_bytes());
    for color in colors {
        payload.extend_from_slice(&color.to_wire_bytes());
    }
    Ok(payload)
}

/// Build the payload for `UPDATEMODE`.
///
/// # Errors
///
/// Returns an error when the encoded mode block is oversized.
pub fn update_mode_payload(mode_index: u32, mode: &ControllerMode) -> Result<Vec<u8>> {
    let mode_bytes = mode.encode()?;
    let size = 8_usize
        .checked_add(mode_bytes.len())
        .ok_or(OpenRgbError::CountOverflow {
            count: mode_bytes.len(),
            element_size: 1,
        })?;
    let mut payload = Vec::with_capacity(size);
    payload.extend_from_slice(
        &u32::try_from(size)
            .map_err(|_| OpenRgbError::PacketTooLarge {
                size,
                max: MAX_PACKET_PAYLOAD_SIZE,
            })?
            .to_le_bytes(),
    );
    payload.extend_from_slice(&mode_index.to_le_bytes());
    payload.extend_from_slice(&mode_bytes);
    Ok(payload)
}

fn checked_color_block_size(prefix_after_size: usize, colors: usize) -> Result<usize> {
    let color_bytes =
        colors
            .checked_mul(RgbColor::WIRE_SIZE)
            .ok_or(OpenRgbError::CountOverflow {
                count: colors,
                element_size: RgbColor::WIRE_SIZE,
            })?;
    4_usize
        .checked_add(prefix_after_size)
        .and_then(|prefix| prefix.checked_add(color_bytes))
        .ok_or(OpenRgbError::CountOverflow {
            count: colors,
            element_size: RgbColor::WIRE_SIZE,
        })
}
