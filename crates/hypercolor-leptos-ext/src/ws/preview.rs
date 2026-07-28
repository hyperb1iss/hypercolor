use bytes::{BufMut, Bytes};
use thiserror::Error;

pub const PREVIEW_FRAME_HEADER_LEN: usize = 14;
pub const WIDE_PREVIEW_FRAME_HEADER_LEN: usize = 19;
pub const WIDE_PREVIEW_FRAME_TAG: u8 = 0x0B;
pub const ZONE_PREVIEW_FRAME_HEADER_LEN: usize = 46;
pub const ZONE_PREVIEW_FRAME_TAG: u8 = 0x08;
pub const WIDE_ZONE_PREVIEW_FRAME_HEADER_LEN: usize = 50;
pub const WIDE_ZONE_PREVIEW_FRAME_TAG: u8 = 0x0C;
pub const SCREEN_ZONES_FRAME_HEADER_LEN: usize = 19;
pub const SCREEN_ZONES_FRAME_TAG: u8 = 0x09;
pub const WIDE_SCREEN_ZONES_FRAME_HEADER_LEN: usize = 23;
pub const WIDE_SCREEN_ZONES_FRAME_TAG: u8 = 0x0E;
pub const INTERACTIVE_PREVIEW_FRAME_TAG: u8 = 0x0A;
pub const INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN: usize = 15;
pub const WIDE_INTERACTIVE_PREVIEW_FRAME_TAG: u8 = 0x0D;
pub const WIDE_INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN: usize = 19;
pub const INTERACTIVE_PREVIEW_ID_MAX_BYTES: usize = 128;
pub const PREVIEW_CHUNK_FRAME_TAG: u8 = 0x0F;
pub const PREVIEW_CHUNK_FIXED_HEADER_LEN: usize = 55;
pub const PREVIEW_CHUNK_SCHEMA: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PreviewFrameChannel {
    Canvas = 0x03,
    ScreenCanvas = 0x05,
    WebViewportCanvas = 0x06,
    DisplayPreview = 0x07,
}

impl PreviewFrameChannel {
    #[must_use]
    pub const fn tag(self) -> u8 {
        self as u8
    }
}

impl TryFrom<u8> for PreviewFrameChannel {
    type Error = PreviewFrameDecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x03 => Ok(Self::Canvas),
            0x05 => Ok(Self::ScreenCanvas),
            0x06 => Ok(Self::WebViewportCanvas),
            0x07 => Ok(Self::DisplayPreview),
            actual => Err(PreviewFrameDecodeError::UnknownChannel { actual }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PreviewPixelFormat {
    Rgb = 0,
    Rgba = 1,
    Jpeg = 2,
}

impl PreviewPixelFormat {
    #[must_use]
    pub const fn tag(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn bytes_per_pixel(self) -> Option<usize> {
        match self {
            Self::Rgb => Some(3),
            Self::Rgba => Some(4),
            Self::Jpeg => None,
        }
    }
}

impl TryFrom<u8> for PreviewPixelFormat {
    type Error = PreviewFrameDecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Rgb),
            1 => Ok(Self::Rgba),
            2 => Ok(Self::Jpeg),
            actual => Err(PreviewFrameDecodeError::UnknownPixelFormat { actual }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewFrame {
    pub channel: PreviewFrameChannel,
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,
    pub payload: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZonePreviewFrame {
    pub scene_id: [u8; 16],
    pub zone_id: [u8; 16],
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,
    pub payload: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractivePreviewFrame {
    pub preview_id: String,
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,
    pub payload: Bytes,
}

impl PreviewFrame {
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        self.header_len()
            .checked_add(self.payload.len())
            .expect("validated preview frame length")
    }

    #[must_use]
    pub fn encode(&self) -> Bytes {
        self.try_encode()
            .expect("validated preview frame allocation")
    }

    pub fn try_encode(&self) -> Result<Bytes, PreviewFrameDecodeError> {
        validate_payload(self.width, self.height, self.format, self.payload.len())?;
        let encoded_len = self
            .header_len()
            .checked_add(self.payload.len())
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        let mut out = Vec::new();
        out.try_reserve_exact(encoded_len)
            .map_err(|_| PreviewFrameDecodeError::AllocationFailed { bytes: encoded_len })?;
        if self.uses_legacy_layout() {
            out.put_u8(self.channel.tag());
        } else {
            out.put_u8(WIDE_PREVIEW_FRAME_TAG);
            out.put_u8(self.channel.tag());
        }
        out.put_u32_le(self.frame_number);
        out.put_u32_le(self.timestamp_ms);
        if self.uses_legacy_layout() {
            out.put_u16_le(u16::try_from(self.width).expect("legacy width fits"));
            out.put_u16_le(u16::try_from(self.height).expect("legacy height fits"));
        } else {
            out.put_u32_le(self.width);
            out.put_u32_le(self.height);
        }
        out.put_u8(self.format.tag());
        out.extend_from_slice(&self.payload);
        Ok(Bytes::from(out))
    }

    #[must_use]
    pub const fn uses_legacy_layout(&self) -> bool {
        self.width <= u16::MAX as u32 && self.height <= u16::MAX as u32
    }

    #[must_use]
    pub const fn header_len(&self) -> usize {
        if self.uses_legacy_layout() {
            PREVIEW_FRAME_HEADER_LEN
        } else {
            WIDE_PREVIEW_FRAME_HEADER_LEN
        }
    }

    pub fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        let header = PreviewFrameHeader::decode(input)?;
        let end = header.end_offset(input.len())?;

        Ok(Self {
            channel: header.channel,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: header.width,
            height: header.height,
            format: header.format,
            payload: Bytes::copy_from_slice(&input[header.payload_offset..end]),
        })
    }

    /// Zero-copy decode for native clients that already hold the message
    /// as `Bytes` — the payload is a refcounted slice of the input, not a
    /// copy. Decode behavior is otherwise identical to [`Self::decode`].
    pub fn decode_bytes(input: &Bytes) -> Result<Self, PreviewFrameDecodeError> {
        let header = PreviewFrameHeader::decode(input)?;
        let end = header.end_offset(input.len())?;

        Ok(Self {
            channel: header.channel,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: header.width,
            height: header.height,
            format: header.format,
            payload: input.slice(header.payload_offset..end),
        })
    }
}

impl ZonePreviewFrame {
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        self.header_len()
            .checked_add(self.payload.len())
            .expect("validated zone preview frame length")
    }

    #[must_use]
    pub fn encode(&self) -> Bytes {
        self.try_encode()
            .expect("validated zone preview frame allocation")
    }

    pub fn try_encode(&self) -> Result<Bytes, PreviewFrameDecodeError> {
        validate_payload(self.width, self.height, self.format, self.payload.len())?;
        let encoded_len = self
            .header_len()
            .checked_add(self.payload.len())
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        let mut out = Vec::new();
        out.try_reserve_exact(encoded_len)
            .map_err(|_| PreviewFrameDecodeError::AllocationFailed { bytes: encoded_len })?;
        out.put_u8(if self.uses_legacy_layout() {
            ZONE_PREVIEW_FRAME_TAG
        } else {
            WIDE_ZONE_PREVIEW_FRAME_TAG
        });
        out.put_u32_le(self.frame_number);
        out.put_u32_le(self.timestamp_ms);
        out.extend_from_slice(&self.scene_id);
        out.extend_from_slice(&self.zone_id);
        if self.uses_legacy_layout() {
            out.put_u16_le(u16::try_from(self.width).expect("legacy width fits"));
            out.put_u16_le(u16::try_from(self.height).expect("legacy height fits"));
        } else {
            out.put_u32_le(self.width);
            out.put_u32_le(self.height);
        }
        out.put_u8(self.format.tag());
        out.extend_from_slice(&self.payload);
        Ok(Bytes::from(out))
    }

    #[must_use]
    pub const fn uses_legacy_layout(&self) -> bool {
        self.width <= u16::MAX as u32 && self.height <= u16::MAX as u32
    }

    #[must_use]
    pub const fn header_len(&self) -> usize {
        if self.uses_legacy_layout() {
            ZONE_PREVIEW_FRAME_HEADER_LEN
        } else {
            WIDE_ZONE_PREVIEW_FRAME_HEADER_LEN
        }
    }

    pub fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        let header = ZonePreviewFrameHeader::decode(input)?;
        let end = header.end_offset(input.len())?;

        Ok(Self {
            scene_id: header.scene_id,
            zone_id: header.zone_id,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: header.width,
            height: header.height,
            format: header.format,
            payload: Bytes::copy_from_slice(&input[header.payload_offset..end]),
        })
    }

    /// Zero-copy decode for native clients that already hold the message
    /// as `Bytes` — see [`PreviewFrame::decode_bytes`].
    pub fn decode_bytes(input: &Bytes) -> Result<Self, PreviewFrameDecodeError> {
        let header = ZonePreviewFrameHeader::decode(input)?;
        let end = header.end_offset(input.len())?;

        Ok(Self {
            scene_id: header.scene_id,
            zone_id: header.zone_id,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: header.width,
            height: header.height,
            format: header.format,
            payload: input.slice(header.payload_offset..end),
        })
    }
}

impl InteractivePreviewFrame {
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        self.prefix_len()
            .checked_add(self.preview_id.len())
            .and_then(|len| len.checked_add(self.payload.len()))
            .expect("validated interactive preview frame length")
    }

    pub fn encode(&self) -> Result<Bytes, PreviewFrameDecodeError> {
        validate_interactive_preview_id(&self.preview_id)?;
        let id_len = u8::try_from(self.preview_id.len()).map_err(|_| {
            PreviewFrameDecodeError::PreviewIdTooLong {
                maximum: INTERACTIVE_PREVIEW_ID_MAX_BYTES,
                actual: self.preview_id.len(),
            }
        })?;
        validate_payload(self.width, self.height, self.format, self.payload.len())?;
        let encoded_len = self
            .prefix_len()
            .checked_add(self.preview_id.len())
            .and_then(|len| len.checked_add(self.payload.len()))
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        let mut out = Vec::new();
        out.try_reserve_exact(encoded_len)
            .map_err(|_| PreviewFrameDecodeError::AllocationFailed { bytes: encoded_len })?;
        out.put_u8(if self.uses_legacy_layout() {
            INTERACTIVE_PREVIEW_FRAME_TAG
        } else {
            WIDE_INTERACTIVE_PREVIEW_FRAME_TAG
        });
        out.put_u8(id_len);
        out.put_u32_le(self.frame_number);
        out.put_u32_le(self.timestamp_ms);
        if self.uses_legacy_layout() {
            out.put_u16_le(u16::try_from(self.width).expect("legacy width fits"));
            out.put_u16_le(u16::try_from(self.height).expect("legacy height fits"));
        } else {
            out.put_u32_le(self.width);
            out.put_u32_le(self.height);
        }
        out.put_u8(self.format.tag());
        out.extend_from_slice(self.preview_id.as_bytes());
        out.extend_from_slice(&self.payload);
        Ok(Bytes::from(out))
    }

    #[must_use]
    pub const fn uses_legacy_layout(&self) -> bool {
        self.width <= u16::MAX as u32 && self.height <= u16::MAX as u32
    }

    #[must_use]
    pub const fn prefix_len(&self) -> usize {
        if self.uses_legacy_layout() {
            INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN
        } else {
            WIDE_INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN
        }
    }

    pub fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        let header = InteractivePreviewFrameHeader::decode(input)?;
        let end = header.end_offset(input.len())?;
        Ok(Self {
            preview_id: header.preview_id,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: header.width,
            height: header.height,
            format: header.format,
            payload: Bytes::copy_from_slice(&input[header.payload_offset..end]),
        })
    }

    pub fn decode_bytes(input: &Bytes) -> Result<Self, PreviewFrameDecodeError> {
        let header = InteractivePreviewFrameHeader::decode(input)?;
        let end = header.end_offset(input.len())?;
        Ok(Self {
            preview_id: header.preview_id,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: header.width,
            height: header.height,
            format: header.format,
            payload: input.slice(header.payload_offset..end),
        })
    }
}

/// Ambilight zone grid frame — the smoothed, color-tuned per-sector colors
/// extracted from screen capture, exactly as screen-reactive effects see
/// them. Payload is row-major RGB, `grid_cols * grid_rows * 3` bytes.
///
/// Wire layout (little endian):
/// tag u8 | frame u32 | timestamp u32 | source_w u16/u32 | source_h u16/u32 |
/// cols u8 | rows u8 | letterbox top/bottom/left/right u8 each | RGB payload
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenZonesFrame {
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub grid_cols: u8,
    pub grid_rows: u8,
    /// Letterbox bars in grid units: top, bottom, left, right.
    pub letterbox: [u8; 4],
    pub payload: Bytes,
}

impl ScreenZonesFrame {
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        self.header_len()
            .checked_add(self.payload.len())
            .expect("validated screen zones frame length")
    }

    #[must_use]
    pub fn encode(&self) -> Bytes {
        self.try_encode()
            .expect("validated screen zones frame allocation")
    }

    pub fn try_encode(&self) -> Result<Bytes, PreviewFrameDecodeError> {
        let payload_len = usize::from(self.grid_cols)
            .checked_mul(usize::from(self.grid_rows))
            .and_then(|zones| zones.checked_mul(3))
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        if self.payload.len() < payload_len {
            return Err(PreviewFrameDecodeError::PayloadTooShort {
                expected: payload_len,
                actual: self.payload.len(),
            });
        }
        let encoded_len = self
            .header_len()
            .checked_add(self.payload.len())
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        let mut out = Vec::new();
        out.try_reserve_exact(encoded_len)
            .map_err(|_| PreviewFrameDecodeError::AllocationFailed { bytes: encoded_len })?;
        out.put_u8(if self.uses_legacy_layout() {
            SCREEN_ZONES_FRAME_TAG
        } else {
            WIDE_SCREEN_ZONES_FRAME_TAG
        });
        out.put_u32_le(self.frame_number);
        out.put_u32_le(self.timestamp_ms);
        if self.uses_legacy_layout() {
            out.put_u16_le(u16::try_from(self.source_width).expect("legacy width fits"));
            out.put_u16_le(u16::try_from(self.source_height).expect("legacy height fits"));
        } else {
            out.put_u32_le(self.source_width);
            out.put_u32_le(self.source_height);
        }
        out.put_u8(self.grid_cols);
        out.put_u8(self.grid_rows);
        out.extend_from_slice(&self.letterbox);
        out.extend_from_slice(&self.payload);
        Ok(Bytes::from(out))
    }

    #[must_use]
    pub const fn uses_legacy_layout(&self) -> bool {
        self.source_width <= u16::MAX as u32 && self.source_height <= u16::MAX as u32
    }

    #[must_use]
    pub const fn header_len(&self) -> usize {
        if self.uses_legacy_layout() {
            SCREEN_ZONES_FRAME_HEADER_LEN
        } else {
            WIDE_SCREEN_ZONES_FRAME_HEADER_LEN
        }
    }

    pub fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        let header = ScreenZonesFrameHeader::decode(input)?;
        let end = header.end_offset(input.len())?;

        Ok(Self {
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            source_width: header.source_width,
            source_height: header.source_height,
            grid_cols: header.grid_cols,
            grid_rows: header.grid_rows,
            letterbox: header.letterbox,
            payload: Bytes::copy_from_slice(&input[header.payload_offset..end]),
        })
    }

    /// RGB color of the zone at `(row, col)`, if in range.
    #[must_use]
    pub fn zone_rgb(&self, row: u8, col: u8) -> Option<[u8; 3]> {
        if row >= self.grid_rows || col >= self.grid_cols {
            return None;
        }
        let offset = (usize::from(row) * usize::from(self.grid_cols) + usize::from(col)) * 3;
        let bytes = self.payload.get(offset..offset + 3)?;
        Some([bytes[0], bytes[1], bytes[2]])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewStreamId {
    Passive(PreviewFrameChannel),
    Zone {
        scene_id: [u8; 16],
        zone_id: [u8; 16],
    },
    Interactive(String),
    ScreenZones,
}

impl PreviewStreamId {
    fn wire_parts(&self) -> Result<(u8, u8, Vec<u8>), PreviewFrameDecodeError> {
        match self {
            Self::Passive(channel) => Ok((0, channel.tag(), Vec::new())),
            Self::Zone { scene_id, zone_id } => {
                let mut identity = Vec::new();
                identity
                    .try_reserve_exact(32)
                    .map_err(|_| PreviewFrameDecodeError::AllocationFailed { bytes: 32 })?;
                identity.extend_from_slice(scene_id);
                identity.extend_from_slice(zone_id);
                Ok((1, ZONE_PREVIEW_FRAME_TAG, identity))
            }
            Self::Interactive(preview_id) => {
                validate_interactive_preview_id(preview_id)?;
                Ok((
                    2,
                    INTERACTIVE_PREVIEW_FRAME_TAG,
                    preview_id.as_bytes().to_vec(),
                ))
            }
            Self::ScreenZones => Ok((3, SCREEN_ZONES_FRAME_TAG, Vec::new())),
        }
    }

    fn decode(kind: u8, channel: u8, identity: &[u8]) -> Result<Self, PreviewChunkError> {
        match kind {
            0 if identity.is_empty() => Ok(Self::Passive(
                PreviewFrameChannel::try_from(channel).map_err(PreviewChunkError::Frame)?,
            )),
            1 if channel == ZONE_PREVIEW_FRAME_TAG && identity.len() == 32 => Ok(Self::Zone {
                scene_id: identity[..16].try_into().expect("identity has 16 bytes"),
                zone_id: identity[16..].try_into().expect("identity has 16 bytes"),
            }),
            2 if channel == INTERACTIVE_PREVIEW_FRAME_TAG => {
                let preview_id = std::str::from_utf8(identity)
                    .map_err(|_| {
                        PreviewChunkError::Frame(PreviewFrameDecodeError::InvalidPreviewIdUtf8)
                    })?
                    .to_owned();
                validate_interactive_preview_id(&preview_id).map_err(PreviewChunkError::Frame)?;
                Ok(Self::Interactive(preview_id))
            }
            3 if channel == SCREEN_ZONES_FRAME_TAG && identity.is_empty() => Ok(Self::ScreenZones),
            _ => Err(PreviewChunkError::InvalidStreamIdentity),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewPublicationMetadata {
    pub stream: PreviewStreamId,
    pub publication_id: u64,
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewChunkFrame {
    pub metadata: PreviewPublicationMetadata,
    pub total_encoded_bytes: u64,
    pub chunk_offset: u64,
    pub chunk_index: u32,
    pub chunk_count: u32,
    pub payload: Bytes,
}

impl PreviewChunkFrame {
    pub fn try_encode(&self) -> Result<Bytes, PreviewChunkError> {
        let (stream_kind, channel, identity) = self
            .metadata
            .stream
            .wire_parts()
            .map_err(PreviewChunkError::Frame)?;
        let identity_len =
            u16::try_from(identity.len()).map_err(|_| PreviewChunkError::InvalidStreamIdentity)?;
        self.validate_layout()?;
        let encoded_len = PREVIEW_CHUNK_FIXED_HEADER_LEN
            .checked_add(identity.len())
            .and_then(|len| len.checked_add(self.payload.len()))
            .ok_or(PreviewChunkError::LengthOverflow)?;
        let mut out = Vec::new();
        out.try_reserve_exact(encoded_len)
            .map_err(|_| PreviewChunkError::AllocationFailed { bytes: encoded_len })?;
        out.put_u8(PREVIEW_CHUNK_FRAME_TAG);
        out.put_u8(PREVIEW_CHUNK_SCHEMA);
        out.put_u8(stream_kind);
        out.put_u8(channel);
        out.put_u8(self.metadata.format.tag());
        out.put_u16_le(identity_len);
        out.put_u64_le(self.metadata.publication_id);
        out.put_u32_le(self.metadata.frame_number);
        out.put_u32_le(self.metadata.timestamp_ms);
        out.put_u32_le(self.metadata.width);
        out.put_u32_le(self.metadata.height);
        out.put_u64_le(self.total_encoded_bytes);
        out.put_u64_le(self.chunk_offset);
        out.put_u32_le(self.chunk_index);
        out.put_u32_le(self.chunk_count);
        out.extend_from_slice(&identity);
        out.extend_from_slice(&self.payload);
        Ok(Bytes::from(out))
    }

    pub fn decode_bytes(input: &Bytes) -> Result<Self, PreviewChunkError> {
        if input.len() < PREVIEW_CHUNK_FIXED_HEADER_LEN {
            return Err(PreviewChunkError::TooShort {
                expected: PREVIEW_CHUNK_FIXED_HEADER_LEN,
                actual: input.len(),
            });
        }
        if input[0] != PREVIEW_CHUNK_FRAME_TAG {
            return Err(PreviewChunkError::UnknownTag { actual: input[0] });
        }
        if input[1] != PREVIEW_CHUNK_SCHEMA {
            return Err(PreviewChunkError::UnknownSchema { actual: input[1] });
        }
        let identity_len = usize::from(u16::from_le_bytes(
            input[5..7].try_into().expect("slice has 2 bytes"),
        ));
        let payload_offset = PREVIEW_CHUNK_FIXED_HEADER_LEN
            .checked_add(identity_len)
            .ok_or(PreviewChunkError::LengthOverflow)?;
        if input.len() < payload_offset {
            return Err(PreviewChunkError::TooShort {
                expected: payload_offset,
                actual: input.len(),
            });
        }
        let stream = PreviewStreamId::decode(
            input[2],
            input[3],
            &input[PREVIEW_CHUNK_FIXED_HEADER_LEN..payload_offset],
        )?;
        let frame = Self {
            metadata: PreviewPublicationMetadata {
                stream,
                publication_id: u64::from_le_bytes(
                    input[7..15].try_into().expect("slice has 8 bytes"),
                ),
                frame_number: u32::from_le_bytes(
                    input[15..19].try_into().expect("slice has 4 bytes"),
                ),
                timestamp_ms: u32::from_le_bytes(
                    input[19..23].try_into().expect("slice has 4 bytes"),
                ),
                width: u32::from_le_bytes(input[23..27].try_into().expect("slice has 4 bytes")),
                height: u32::from_le_bytes(input[27..31].try_into().expect("slice has 4 bytes")),
                format: PreviewPixelFormat::try_from(input[4]).map_err(PreviewChunkError::Frame)?,
            },
            total_encoded_bytes: u64::from_le_bytes(
                input[31..39].try_into().expect("slice has 8 bytes"),
            ),
            chunk_offset: u64::from_le_bytes(input[39..47].try_into().expect("slice has 8 bytes")),
            chunk_index: u32::from_le_bytes(input[47..51].try_into().expect("slice has 4 bytes")),
            chunk_count: u32::from_le_bytes(input[51..55].try_into().expect("slice has 4 bytes")),
            payload: input.slice(payload_offset..),
        };
        frame.validate_layout()?;
        Ok(frame)
    }

    fn validate_layout(&self) -> Result<(), PreviewChunkError> {
        if self.total_encoded_bytes == 0 || self.chunk_count == 0 {
            return Err(PreviewChunkError::InvalidLayout);
        }
        if self.chunk_index >= self.chunk_count || self.payload.is_empty() {
            return Err(PreviewChunkError::InvalidLayout);
        }
        let payload_len =
            u64::try_from(self.payload.len()).map_err(|_| PreviewChunkError::LengthOverflow)?;
        let end = self
            .chunk_offset
            .checked_add(payload_len)
            .ok_or(PreviewChunkError::LengthOverflow)?;
        if end > self.total_encoded_bytes {
            return Err(PreviewChunkError::InvalidLayout);
        }
        if self.chunk_index + 1 == self.chunk_count {
            if end != self.total_encoded_bytes {
                return Err(PreviewChunkError::InvalidLayout);
            }
        } else if end >= self.total_encoded_bytes {
            return Err(PreviewChunkError::InvalidLayout);
        }
        Ok(())
    }
}

pub fn split_preview_publication(
    encoded: &Bytes,
    metadata: &PreviewPublicationMetadata,
    max_message_bytes: usize,
) -> Result<Vec<Bytes>, PreviewChunkError> {
    let (_, _, identity) = metadata
        .stream
        .wire_parts()
        .map_err(PreviewChunkError::Frame)?;
    let envelope_bytes = PREVIEW_CHUNK_FIXED_HEADER_LEN
        .checked_add(identity.len())
        .ok_or(PreviewChunkError::LengthOverflow)?;
    let payload_capacity = max_message_bytes
        .checked_sub(envelope_bytes)
        .filter(|capacity| *capacity > 0)
        .ok_or(PreviewChunkError::MessageBudgetTooSmall {
            required: envelope_bytes.saturating_add(1),
            actual: max_message_bytes,
        })?;
    if encoded.is_empty() {
        return Err(PreviewChunkError::InvalidLayout);
    }
    let chunk_count = encoded.len().div_ceil(payload_capacity);
    let chunk_count_u32 =
        u32::try_from(chunk_count).map_err(|_| PreviewChunkError::LengthOverflow)?;
    let total_encoded_bytes =
        u64::try_from(encoded.len()).map_err(|_| PreviewChunkError::LengthOverflow)?;
    let chunk_index_bytes = chunk_count
        .checked_mul(std::mem::size_of::<Bytes>())
        .ok_or(PreviewChunkError::LengthOverflow)?;
    let mut chunks = Vec::new();
    chunks
        .try_reserve_exact(chunk_count)
        .map_err(|_| PreviewChunkError::AllocationFailed {
            bytes: chunk_index_bytes,
        })?;
    for (index, payload) in encoded.chunks(payload_capacity).enumerate() {
        let offset = index
            .checked_mul(payload_capacity)
            .ok_or(PreviewChunkError::LengthOverflow)?;
        let chunk = PreviewChunkFrame {
            metadata: metadata.clone(),
            total_encoded_bytes,
            chunk_offset: u64::try_from(offset).map_err(|_| PreviewChunkError::LengthOverflow)?,
            chunk_index: u32::try_from(index).map_err(|_| PreviewChunkError::LengthOverflow)?,
            chunk_count: chunk_count_u32,
            payload: Bytes::copy_from_slice(payload),
        };
        chunks.push(chunk.try_encode()?);
    }
    Ok(chunks)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewReassemblyLimits {
    pub max_publication_bytes: usize,
    pub max_connection_bytes: usize,
    pub max_streams: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReassembledPreviewPublication {
    pub metadata: PreviewPublicationMetadata,
    pub encoded: Bytes,
}

#[derive(Debug)]
struct PartialPreviewPublication {
    metadata: PreviewPublicationMetadata,
    total_encoded_bytes: usize,
    chunk_count: u32,
    next_chunk_index: u32,
    bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct PreviewChunkReassembler {
    limits: PreviewReassemblyLimits,
    partials: Vec<PartialPreviewPublication>,
    completed: Vec<(PreviewStreamId, u64)>,
    superseded_publications: u64,
}

impl PreviewChunkReassembler {
    #[must_use]
    pub const fn new(limits: PreviewReassemblyLimits) -> Self {
        Self {
            limits,
            partials: Vec::new(),
            completed: Vec::new(),
            superseded_publications: 0,
        }
    }

    pub fn push(
        &mut self,
        encoded_chunk: &Bytes,
    ) -> Result<Option<ReassembledPreviewPublication>, PreviewChunkError> {
        let chunk = PreviewChunkFrame::decode_bytes(encoded_chunk)?;
        if self.completed.iter().any(|(stream, publication_id)| {
            stream == &chunk.metadata.stream && *publication_id == chunk.metadata.publication_id
        }) {
            return Err(PreviewChunkError::DuplicateChunk);
        }
        let total_encoded_bytes = usize::try_from(chunk.total_encoded_bytes)
            .map_err(|_| PreviewChunkError::LengthOverflow)?;
        if total_encoded_bytes > self.limits.max_publication_bytes {
            return Err(PreviewChunkError::PublicationBudgetExceeded {
                requested: total_encoded_bytes,
                limit: self.limits.max_publication_bytes,
            });
        }

        let stream_index = self
            .partials
            .iter()
            .position(|partial| partial.metadata.stream == chunk.metadata.stream);
        let partial_index = match stream_index {
            Some(index)
                if self.partials[index].metadata.publication_id
                    == chunk.metadata.publication_id =>
            {
                index
            }
            Some(index) => {
                if chunk.chunk_index != 0 || chunk.chunk_offset != 0 {
                    return Err(PreviewChunkError::MissingPublicationStart);
                }
                self.partials.swap_remove(index);
                self.superseded_publications = self.superseded_publications.saturating_add(1);
                self.admit_partial(&chunk, total_encoded_bytes)?
            }
            None => self.admit_partial(&chunk, total_encoded_bytes)?,
        };

        let partial = &mut self.partials[partial_index];
        if partial.metadata != chunk.metadata
            || partial.total_encoded_bytes != total_encoded_bytes
            || partial.chunk_count != chunk.chunk_count
        {
            return Err(PreviewChunkError::MetadataChanged);
        }
        if chunk.chunk_index < partial.next_chunk_index {
            return Err(PreviewChunkError::DuplicateChunk);
        }
        let expected_offset =
            u64::try_from(partial.bytes.len()).map_err(|_| PreviewChunkError::LengthOverflow)?;
        if chunk.chunk_index != partial.next_chunk_index || chunk.chunk_offset != expected_offset {
            return Err(PreviewChunkError::NonContiguousChunk);
        }
        partial.bytes.extend_from_slice(&chunk.payload);
        partial.next_chunk_index = partial
            .next_chunk_index
            .checked_add(1)
            .ok_or(PreviewChunkError::LengthOverflow)?;

        if partial.next_chunk_index != partial.chunk_count {
            return Ok(None);
        }
        if partial.bytes.len() != partial.total_encoded_bytes {
            return Err(PreviewChunkError::NonContiguousChunk);
        }
        let completed = self.partials.swap_remove(partial_index);
        self.completed
            .retain(|(stream, _)| stream != &completed.metadata.stream);
        if self.completed.len() >= self.limits.max_streams && !self.completed.is_empty() {
            self.completed.swap_remove(0);
        }
        self.completed.push((
            completed.metadata.stream.clone(),
            completed.metadata.publication_id,
        ));
        Ok(Some(ReassembledPreviewPublication {
            metadata: completed.metadata,
            encoded: Bytes::from(completed.bytes),
        }))
    }

    fn admit_partial(
        &mut self,
        chunk: &PreviewChunkFrame,
        total_encoded_bytes: usize,
    ) -> Result<usize, PreviewChunkError> {
        if chunk.chunk_index != 0 || chunk.chunk_offset != 0 {
            return Err(PreviewChunkError::MissingPublicationStart);
        }
        if self.partials.len() >= self.limits.max_streams {
            return Err(PreviewChunkError::StreamBudgetExceeded {
                limit: self.limits.max_streams,
            });
        }
        let reserved = self
            .partials
            .iter()
            .try_fold(0_usize, |total, partial| {
                total.checked_add(partial.total_encoded_bytes)
            })
            .ok_or(PreviewChunkError::LengthOverflow)?;
        let requested = reserved
            .checked_add(total_encoded_bytes)
            .ok_or(PreviewChunkError::LengthOverflow)?;
        if requested > self.limits.max_connection_bytes {
            return Err(PreviewChunkError::ConnectionBudgetExceeded {
                requested,
                limit: self.limits.max_connection_bytes,
            });
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(total_encoded_bytes).map_err(|_| {
            PreviewChunkError::AllocationFailed {
                bytes: total_encoded_bytes,
            }
        })?;
        self.partials.push(PartialPreviewPublication {
            metadata: chunk.metadata.clone(),
            total_encoded_bytes,
            chunk_count: chunk.chunk_count,
            next_chunk_index: 0,
            bytes,
        });
        Ok(self.partials.len() - 1)
    }

    #[must_use]
    pub const fn superseded_publications(&self) -> u64 {
        self.superseded_publications
    }

    #[must_use]
    pub fn reserved_bytes(&self) -> usize {
        self.partials
            .iter()
            .map(|partial| partial.total_encoded_bytes)
            .sum()
    }

    #[must_use]
    pub fn partial_count(&self) -> usize {
        self.partials.len()
    }

    pub fn clear(&mut self) {
        self.partials.clear();
        self.completed.clear();
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PreviewChunkError {
    #[error(transparent)]
    Frame(#[from] PreviewFrameDecodeError),
    #[error("preview chunk is too short: expected at least {expected} bytes, got {actual}")]
    TooShort { expected: usize, actual: usize },
    #[error("unknown preview chunk tag {actual:#04x}")]
    UnknownTag { actual: u8 },
    #[error("unknown preview chunk schema {actual}")]
    UnknownSchema { actual: u8 },
    #[error("preview chunk stream identity is invalid")]
    InvalidStreamIdentity,
    #[error("preview chunk layout is invalid")]
    InvalidLayout,
    #[error("preview chunk length overflows the platform address space")]
    LengthOverflow,
    #[error("preview chunk allocation failed for {bytes} bytes")]
    AllocationFailed { bytes: usize },
    #[error("preview chunk message budget needs {required} bytes, got {actual}")]
    MessageBudgetTooSmall { required: usize, actual: usize },
    #[error("preview publication needs {requested} bytes; limit is {limit}")]
    PublicationBudgetExceeded { requested: usize, limit: usize },
    #[error("preview connection needs {requested} bytes; limit is {limit}")]
    ConnectionBudgetExceeded { requested: usize, limit: usize },
    #[error("preview reassembly stream limit is {limit}")]
    StreamBudgetExceeded { limit: usize },
    #[error("preview chunk arrived without publication start")]
    MissingPublicationStart,
    #[error("preview chunk duplicates already received data")]
    DuplicateChunk,
    #[error("preview chunks are not contiguous")]
    NonContiguousChunk,
    #[error("preview chunk metadata changed within a publication")]
    MetadataChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScreenZonesFrameHeader {
    frame_number: u32,
    timestamp_ms: u32,
    source_width: u32,
    source_height: u32,
    grid_cols: u8,
    grid_rows: u8,
    letterbox: [u8; 4],
    payload_offset: usize,
}

impl ScreenZonesFrameHeader {
    fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        let wide = input.first() == Some(&WIDE_SCREEN_ZONES_FRAME_TAG);
        let header_len = if wide {
            WIDE_SCREEN_ZONES_FRAME_HEADER_LEN
        } else {
            SCREEN_ZONES_FRAME_HEADER_LEN
        };
        if input.len() < header_len {
            return Err(PreviewFrameDecodeError::TooShort {
                expected: header_len,
                actual: input.len(),
            });
        }
        if input[0] != SCREEN_ZONES_FRAME_TAG && !wide {
            return Err(PreviewFrameDecodeError::UnknownChannel { actual: input[0] });
        }

        let (source_width, source_height, grid_offset) = if wide {
            (
                u32::from_le_bytes(input[9..13].try_into().expect("slice has 4 bytes")),
                u32::from_le_bytes(input[13..17].try_into().expect("slice has 4 bytes")),
                17,
            )
        } else {
            (
                u32::from(u16::from_le_bytes(
                    input[9..11].try_into().expect("slice has 2 bytes"),
                )),
                u32::from(u16::from_le_bytes(
                    input[11..13].try_into().expect("slice has 2 bytes"),
                )),
                13,
            )
        };

        Ok(Self {
            frame_number: u32::from_le_bytes(input[1..5].try_into().expect("slice has 4 bytes")),
            timestamp_ms: u32::from_le_bytes(input[5..9].try_into().expect("slice has 4 bytes")),
            source_width,
            source_height,
            grid_cols: input[grid_offset],
            grid_rows: input[grid_offset + 1],
            letterbox: input[grid_offset + 2..grid_offset + 6]
                .try_into()
                .expect("slice has 4 bytes"),
            payload_offset: header_len,
        })
    }

    fn end_offset(&self, input_len: usize) -> Result<usize, PreviewFrameDecodeError> {
        let payload_len = usize::from(self.grid_cols)
            .checked_mul(usize::from(self.grid_rows))
            .and_then(|zones| zones.checked_mul(3))
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        let end = self
            .payload_offset
            .checked_add(payload_len)
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        if input_len < end {
            return Err(PreviewFrameDecodeError::PayloadTooShort {
                expected: payload_len,
                actual: input_len.saturating_sub(self.payload_offset),
            });
        }
        Ok(end)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PreviewFrameHeader {
    channel: PreviewFrameChannel,
    frame_number: u32,
    timestamp_ms: u32,
    width: u32,
    height: u32,
    format: PreviewPixelFormat,
    payload_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ZonePreviewFrameHeader {
    scene_id: [u8; 16],
    zone_id: [u8; 16],
    frame_number: u32,
    timestamp_ms: u32,
    width: u32,
    height: u32,
    format: PreviewPixelFormat,
    payload_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InteractivePreviewFrameHeader {
    preview_id: String,
    frame_number: u32,
    timestamp_ms: u32,
    width: u32,
    height: u32,
    format: PreviewPixelFormat,
    payload_offset: usize,
}

impl PreviewFrameHeader {
    fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        let wide = input.first() == Some(&WIDE_PREVIEW_FRAME_TAG);
        let header_len = if wide {
            WIDE_PREVIEW_FRAME_HEADER_LEN
        } else {
            PREVIEW_FRAME_HEADER_LEN
        };
        if input.len() < header_len {
            return Err(PreviewFrameDecodeError::TooShort {
                expected: header_len,
                actual: input.len(),
            });
        }

        let metadata_offset = usize::from(wide);
        let dimensions_offset = 9 + metadata_offset;
        let (width, height, format_offset) = if wide {
            (
                u32::from_le_bytes(
                    input[dimensions_offset..dimensions_offset + 4]
                        .try_into()
                        .expect("slice has 4 bytes"),
                ),
                u32::from_le_bytes(
                    input[dimensions_offset + 4..dimensions_offset + 8]
                        .try_into()
                        .expect("slice has 4 bytes"),
                ),
                dimensions_offset + 8,
            )
        } else {
            (
                u32::from(u16::from_le_bytes(
                    input[dimensions_offset..dimensions_offset + 2]
                        .try_into()
                        .expect("slice has 2 bytes"),
                )),
                u32::from(u16::from_le_bytes(
                    input[dimensions_offset + 2..dimensions_offset + 4]
                        .try_into()
                        .expect("slice has 2 bytes"),
                )),
                dimensions_offset + 4,
            )
        };

        Ok(Self {
            channel: PreviewFrameChannel::try_from(input[metadata_offset])?,
            frame_number: u32::from_le_bytes(
                input[1 + metadata_offset..5 + metadata_offset]
                    .try_into()
                    .expect("slice has 4 bytes"),
            ),
            timestamp_ms: u32::from_le_bytes(
                input[5 + metadata_offset..9 + metadata_offset]
                    .try_into()
                    .expect("slice has 4 bytes"),
            ),
            width,
            height,
            format: PreviewPixelFormat::try_from(input[format_offset])?,
            payload_offset: header_len,
        })
    }

    fn end_offset(&self, input_len: usize) -> Result<usize, PreviewFrameDecodeError> {
        let payload_len = match self.format.bytes_per_pixel() {
            Some(bytes_per_pixel) => raw_payload_len(self.width, self.height, bytes_per_pixel)?,
            None => input_len - self.payload_offset,
        };
        let end = self
            .payload_offset
            .checked_add(payload_len)
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        if input_len < end {
            return Err(PreviewFrameDecodeError::PayloadTooShort {
                expected: payload_len,
                actual: input_len.saturating_sub(self.payload_offset),
            });
        }
        Ok(end)
    }
}

impl ZonePreviewFrameHeader {
    fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        let wide = input.first() == Some(&WIDE_ZONE_PREVIEW_FRAME_TAG);
        let header_len = if wide {
            WIDE_ZONE_PREVIEW_FRAME_HEADER_LEN
        } else {
            ZONE_PREVIEW_FRAME_HEADER_LEN
        };
        if input.len() < header_len {
            return Err(PreviewFrameDecodeError::TooShort {
                expected: header_len,
                actual: input.len(),
            });
        }
        if input[0] != ZONE_PREVIEW_FRAME_TAG && !wide {
            return Err(PreviewFrameDecodeError::UnknownChannel { actual: input[0] });
        }

        let (width, height, format_offset) = if wide {
            (
                u32::from_le_bytes(input[41..45].try_into().expect("slice has 4 bytes")),
                u32::from_le_bytes(input[45..49].try_into().expect("slice has 4 bytes")),
                49,
            )
        } else {
            (
                u32::from(u16::from_le_bytes(
                    input[41..43].try_into().expect("slice has 2 bytes"),
                )),
                u32::from(u16::from_le_bytes(
                    input[43..45].try_into().expect("slice has 2 bytes"),
                )),
                45,
            )
        };

        Ok(Self {
            frame_number: u32::from_le_bytes(input[1..5].try_into().expect("slice has 4 bytes")),
            timestamp_ms: u32::from_le_bytes(input[5..9].try_into().expect("slice has 4 bytes")),
            scene_id: input[9..25].try_into().expect("slice has 16 bytes"),
            zone_id: input[25..41].try_into().expect("slice has 16 bytes"),
            width,
            height,
            format: PreviewPixelFormat::try_from(input[format_offset])?,
            payload_offset: header_len,
        })
    }

    fn end_offset(&self, input_len: usize) -> Result<usize, PreviewFrameDecodeError> {
        let payload_len = match self.format.bytes_per_pixel() {
            Some(bytes_per_pixel) => raw_payload_len(self.width, self.height, bytes_per_pixel)?,
            None => input_len - self.payload_offset,
        };
        let end = self
            .payload_offset
            .checked_add(payload_len)
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        if input_len < end {
            return Err(PreviewFrameDecodeError::PayloadTooShort {
                expected: payload_len,
                actual: input_len.saturating_sub(self.payload_offset),
            });
        }
        Ok(end)
    }
}

impl InteractivePreviewFrameHeader {
    fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        let wide = input.first() == Some(&WIDE_INTERACTIVE_PREVIEW_FRAME_TAG);
        let prefix_len = if wide {
            WIDE_INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN
        } else {
            INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN
        };
        if input.len() < prefix_len {
            return Err(PreviewFrameDecodeError::TooShort {
                expected: prefix_len,
                actual: input.len(),
            });
        }
        if input[0] != INTERACTIVE_PREVIEW_FRAME_TAG && !wide {
            return Err(PreviewFrameDecodeError::UnknownChannel { actual: input[0] });
        }

        let id_len = usize::from(input[1]);
        if id_len == 0 {
            return Err(PreviewFrameDecodeError::EmptyPreviewId);
        }
        if id_len > INTERACTIVE_PREVIEW_ID_MAX_BYTES {
            return Err(PreviewFrameDecodeError::PreviewIdTooLong {
                maximum: INTERACTIVE_PREVIEW_ID_MAX_BYTES,
                actual: id_len,
            });
        }
        let payload_offset = prefix_len
            .checked_add(id_len)
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        if input.len() < payload_offset {
            return Err(PreviewFrameDecodeError::TooShort {
                expected: payload_offset,
                actual: input.len(),
            });
        }
        let preview_id = std::str::from_utf8(&input[prefix_len..payload_offset])
            .map_err(|_| PreviewFrameDecodeError::InvalidPreviewIdUtf8)?
            .to_owned();
        validate_interactive_preview_id(&preview_id)?;

        let (width, height, format_offset) = if wide {
            (
                u32::from_le_bytes(input[10..14].try_into().expect("slice has 4 bytes")),
                u32::from_le_bytes(input[14..18].try_into().expect("slice has 4 bytes")),
                18,
            )
        } else {
            (
                u32::from(u16::from_le_bytes(
                    input[10..12].try_into().expect("slice has 2 bytes"),
                )),
                u32::from(u16::from_le_bytes(
                    input[12..14].try_into().expect("slice has 2 bytes"),
                )),
                14,
            )
        };

        Ok(Self {
            preview_id,
            frame_number: u32::from_le_bytes(input[2..6].try_into().expect("slice has 4 bytes")),
            timestamp_ms: u32::from_le_bytes(input[6..10].try_into().expect("slice has 4 bytes")),
            width,
            height,
            format: PreviewPixelFormat::try_from(input[format_offset])?,
            payload_offset,
        })
    }

    fn end_offset(&self, input_len: usize) -> Result<usize, PreviewFrameDecodeError> {
        let payload_len = match self.format.bytes_per_pixel() {
            Some(bytes_per_pixel) => raw_payload_len(self.width, self.height, bytes_per_pixel)?,
            None => input_len - self.payload_offset,
        };
        let end = self
            .payload_offset
            .checked_add(payload_len)
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        if input_len < end {
            return Err(PreviewFrameDecodeError::PayloadTooShort {
                expected: payload_len,
                actual: input_len.saturating_sub(self.payload_offset),
            });
        }
        Ok(end)
    }
}

#[cfg(feature = "ws-client-wasm")]
#[derive(Debug, Clone)]
pub struct PreviewFrameView {
    pub channel: PreviewFrameChannel,
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,
    pub payload: js_sys::Uint8Array,
}

#[cfg(feature = "ws-client-wasm")]
#[derive(Debug, Clone)]
pub struct ZonePreviewFrameView {
    pub scene_id: [u8; 16],
    pub zone_id: [u8; 16],
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,
    pub payload: js_sys::Uint8Array,
}

#[cfg(feature = "ws-client-wasm")]
#[derive(Debug, Clone)]
pub struct InteractivePreviewFrameView {
    pub preview_id: String,
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,
    pub payload: js_sys::Uint8Array,
}

#[cfg(feature = "ws-client-wasm")]
impl PreviewFrameView {
    pub fn decode_array_buffer(
        buffer: &js_sys::ArrayBuffer,
    ) -> Result<Self, PreviewFrameDecodeError> {
        let data = js_sys::Uint8Array::new(buffer);
        let header_len = if data.get_index(0) == WIDE_PREVIEW_FRAME_TAG {
            WIDE_PREVIEW_FRAME_HEADER_LEN
        } else {
            PREVIEW_FRAME_HEADER_LEN
        };
        let header_bytes = data.subarray(0, header_len as u32).to_vec();
        let header = PreviewFrameHeader::decode(&header_bytes)?;
        let end = header.end_offset(data.length() as usize)?;

        Ok(Self {
            channel: header.channel,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: header.width,
            height: header.height,
            format: header.format,
            payload: data.subarray(header.payload_offset as u32, end as u32),
        })
    }

    #[must_use]
    pub fn pixel_count(&self) -> usize {
        let Ok(width) = usize::try_from(self.width) else {
            return 0;
        };
        let Ok(height) = usize::try_from(self.height) else {
            return 0;
        };
        width.checked_mul(height).unwrap_or(0)
    }

    #[must_use]
    pub fn rgba_at(&self, pixel_index: usize) -> Option<[u8; 4]> {
        let bytes_per_pixel = self.format.bytes_per_pixel()?;
        let offset = u32::try_from(pixel_index.checked_mul(bytes_per_pixel)?).ok()?;
        let last_component = offset.checked_add(match self.format {
            PreviewPixelFormat::Rgb => 2,
            PreviewPixelFormat::Rgba => 3,
            PreviewPixelFormat::Jpeg => return None,
        })?;
        if last_component >= self.payload.length() {
            return None;
        }

        Some(match self.format {
            PreviewPixelFormat::Rgb => [
                self.payload.get_index(offset),
                self.payload.get_index(offset + 1),
                self.payload.get_index(offset + 2),
                255,
            ],
            PreviewPixelFormat::Rgba => [
                self.payload.get_index(offset),
                self.payload.get_index(offset + 1),
                self.payload.get_index(offset + 2),
                self.payload.get_index(offset + 3),
            ],
            PreviewPixelFormat::Jpeg => return None,
        })
    }

    #[must_use]
    pub const fn pixels_js(&self) -> &js_sys::Uint8Array {
        &self.payload
    }

    /// Copy the whole frame into a tightly-packed RGBA byte vector.
    ///
    /// One `copy_to` boundary crossing instead of a `get_index` call per
    /// component — use this for full-frame reads (encoding, palette work).
    /// Returns `None` for non-raw payloads (JPEG).
    #[must_use]
    pub fn to_rgba_vec(&self) -> Option<Vec<u8>> {
        let len = self.payload.length() as usize;
        match self.format {
            PreviewPixelFormat::Rgba => {
                let mut rgba = vec![0_u8; len];
                self.payload.copy_to(&mut rgba);
                Some(rgba)
            }
            PreviewPixelFormat::Rgb => {
                let mut rgb = vec![0_u8; len];
                self.payload.copy_to(&mut rgb);
                let rgba_len = (len / 3).checked_mul(4)?;
                let mut rgba = Vec::new();
                rgba.try_reserve_exact(rgba_len).ok()?;
                rgba.resize(rgba_len, 0);
                for (src, dst) in rgb.chunks_exact(3).zip(rgba.chunks_exact_mut(4)) {
                    dst[0] = src[0];
                    dst[1] = src[1];
                    dst[2] = src[2];
                    dst[3] = 255;
                }
                Some(rgba)
            }
            PreviewPixelFormat::Jpeg => None,
        }
    }

    #[must_use]
    pub const fn pixel_format(&self) -> PreviewPixelFormat {
        self.format
    }
}

#[cfg(feature = "ws-client-wasm")]
impl ZonePreviewFrameView {
    pub fn decode_array_buffer(
        buffer: &js_sys::ArrayBuffer,
    ) -> Result<Self, PreviewFrameDecodeError> {
        let data = js_sys::Uint8Array::new(buffer);
        let header_len = if data.get_index(0) == WIDE_ZONE_PREVIEW_FRAME_TAG {
            WIDE_ZONE_PREVIEW_FRAME_HEADER_LEN
        } else {
            ZONE_PREVIEW_FRAME_HEADER_LEN
        };
        let header_bytes = data.subarray(0, header_len as u32).to_vec();
        let header = ZonePreviewFrameHeader::decode(&header_bytes)?;
        let end = header.end_offset(data.length() as usize)?;

        Ok(Self {
            scene_id: header.scene_id,
            zone_id: header.zone_id,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: header.width,
            height: header.height,
            format: header.format,
            payload: data.subarray(header.payload_offset as u32, end as u32),
        })
    }
}

#[cfg(feature = "ws-client-wasm")]
impl InteractivePreviewFrameView {
    pub fn decode_array_buffer(
        buffer: &js_sys::ArrayBuffer,
    ) -> Result<Self, PreviewFrameDecodeError> {
        let data = js_sys::Uint8Array::new(buffer);
        let prefix_len = if data.get_index(0) == WIDE_INTERACTIVE_PREVIEW_FRAME_TAG {
            WIDE_INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN
        } else {
            INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN
        };
        let prefix = data.subarray(0, prefix_len as u32).to_vec();
        if prefix.len() < prefix_len {
            return Err(PreviewFrameDecodeError::TooShort {
                expected: prefix_len,
                actual: prefix.len(),
            });
        }
        let header_len = prefix_len
            .checked_add(usize::from(prefix[1]))
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        let header_bytes = data.subarray(0, header_len as u32).to_vec();
        let header = InteractivePreviewFrameHeader::decode(&header_bytes)?;
        let end = header.end_offset(data.length() as usize)?;

        Ok(Self {
            preview_id: header.preview_id,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: header.width,
            height: header.height,
            format: header.format,
            payload: data.subarray(header.payload_offset as u32, end as u32),
        })
    }
}

#[cfg(feature = "ws-client-wasm")]
impl ScreenZonesFrame {
    pub fn decode_array_buffer(
        buffer: &js_sys::ArrayBuffer,
    ) -> Result<Self, PreviewFrameDecodeError> {
        let data = js_sys::Uint8Array::new(buffer).to_vec();
        Self::decode(&data)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PreviewFrameDecodeError {
    #[error("preview frame is too short: expected at least {expected} bytes, got {actual}")]
    TooShort { expected: usize, actual: usize },
    #[error("unknown preview frame channel tag {actual:#04x}")]
    UnknownChannel { actual: u8 },
    #[error("unknown preview pixel format tag {actual:#04x}")]
    UnknownPixelFormat { actual: u8 },
    #[error("preview frame dimensions overflow payload size")]
    DimensionsOverflow,
    #[error("preview frame allocation failed for {bytes} bytes")]
    AllocationFailed { bytes: usize },
    #[error("preview frame payload is too short: expected {expected} bytes, got {actual}")]
    PayloadTooShort { expected: usize, actual: usize },
    #[error("interactive preview id cannot be empty")]
    EmptyPreviewId,
    #[error("interactive preview id exceeds {maximum} bytes: got {actual}")]
    PreviewIdTooLong { maximum: usize, actual: usize },
    #[error("interactive preview id is not valid UTF-8")]
    InvalidPreviewIdUtf8,
    #[error("interactive preview id contains a control character")]
    InvalidPreviewIdCharacter,
}

fn validate_interactive_preview_id(id: &str) -> Result<(), PreviewFrameDecodeError> {
    if id.is_empty() {
        return Err(PreviewFrameDecodeError::EmptyPreviewId);
    }
    if id.len() > INTERACTIVE_PREVIEW_ID_MAX_BYTES {
        return Err(PreviewFrameDecodeError::PreviewIdTooLong {
            maximum: INTERACTIVE_PREVIEW_ID_MAX_BYTES,
            actual: id.len(),
        });
    }
    if id.chars().any(char::is_control) {
        return Err(PreviewFrameDecodeError::InvalidPreviewIdCharacter);
    }
    Ok(())
}

fn raw_payload_len(
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
) -> Result<usize, PreviewFrameDecodeError> {
    usize::try_from(width)
        .map_err(|_| PreviewFrameDecodeError::DimensionsOverflow)?
        .checked_mul(
            usize::try_from(height).map_err(|_| PreviewFrameDecodeError::DimensionsOverflow)?,
        )
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .ok_or(PreviewFrameDecodeError::DimensionsOverflow)
}

fn validate_payload(
    width: u32,
    height: u32,
    format: PreviewPixelFormat,
    actual: usize,
) -> Result<(), PreviewFrameDecodeError> {
    if let Some(bytes_per_pixel) = format.bytes_per_pixel() {
        let expected = raw_payload_len(width, height, bytes_per_pixel)?;
        if actual < expected {
            return Err(PreviewFrameDecodeError::PayloadTooShort { expected, actual });
        }
    }
    Ok(())
}
