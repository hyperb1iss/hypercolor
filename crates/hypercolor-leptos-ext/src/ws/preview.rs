use std::collections::HashMap;
use std::sync::Arc;

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
pub const DISPLAY_PREVIEW_FRAME_TAG: u8 = 0x07;
pub const DISPLAY_PREVIEW_FRAME_PREFIX_LEN: usize = 15;
pub const WIDE_DISPLAY_PREVIEW_FRAME_TAG: u8 = 0x12;
pub const WIDE_DISPLAY_PREVIEW_FRAME_PREFIX_LEN: usize = 19;
pub const DISPLAY_PREVIEW_ID_MAX_BYTES: usize = 128;
pub const PREVIEW_CHUNK_FRAME_TAG: u8 = 0x0F;
pub const PREVIEW_CHUNK_FIXED_HEADER_LEN: usize = 55;
pub const PREVIEW_MIN_MESSAGE_BYTES: usize =
    PREVIEW_CHUNK_FIXED_HEADER_LEN + INTERACTIVE_PREVIEW_ID_MAX_BYTES + 1;
pub const PREVIEW_CHUNK_SCHEMA: u8 = 1;
pub const PREVIEW_CANCEL_FRAME_TAG: u8 = 0x10;
pub const PREVIEW_CANCEL_FIXED_HEADER_LEN: usize = 14;
pub const PREVIEW_CANCEL_SCHEMA: u8 = 1;
pub const EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN: usize = 41;
pub const EXTENDED_SCREEN_ZONES_FRAME_TAG: u8 = 0x11;
pub const DEFAULT_PREVIEW_MAX_DECODED_PUBLICATION_BYTES: usize = 512 * 1024 * 1024;
pub const DEFAULT_PREVIEW_MAX_ENCODED_PUBLICATION_BYTES: usize =
    DEFAULT_PREVIEW_MAX_DECODED_PUBLICATION_BYTES + 64 * 1024;
pub const DEFAULT_PREVIEW_MAX_CONNECTION_BYTES: usize =
    DEFAULT_PREVIEW_MAX_ENCODED_PUBLICATION_BYTES * 2;
pub const DEFAULT_PREVIEW_MAX_IDLE_MS: u64 = 5_000;
pub const DEFAULT_PREVIEW_MAX_MESSAGE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_PREVIEW_MAX_REASSEMBLY_STATE_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_PREVIEW_MAX_TOMBSTONE_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_PREVIEW_MAX_SENDER_STATE_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_PREVIEW_MAX_CURSOR_STATE_BYTES: usize = 8 * 1024 * 1024;

/// The passive preview surfaces: one canvas per connection, no identity
/// beyond the channel byte. Display preview left this family when it
/// became keyed by device — its frames carry the device id, so they
/// have their own layout rather than a channel discriminant.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
#[repr(u8)]
pub enum PreviewFrameChannel {
    Canvas = 0x03,
    ScreenCanvas = 0x05,
    WebViewportCanvas = 0x06,
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

/// One display device's live output frame.
///
/// The device id rides in the header because `display_preview` is keyed
/// by device: a connection following three displays receives three
/// interleaved streams, and the frame has to say which display it came
/// from rather than leaving the client to guess from resolution.
///
/// Wire layout (little endian):
/// tag u8 | device_id_len u8 | frame u32 | timestamp u32 | width u16 |
/// height u16 | format u8 | device_id utf-8 | payload
///
/// The wide layout swaps the u16 dimensions for u32 under its own tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayPreviewFrame {
    pub device_id: String,
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,
    pub payload: Bytes,
}

impl DisplayPreviewFrame {
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        self.prefix_len()
            .checked_add(self.device_id.len())
            .and_then(|len| len.checked_add(self.payload.len()))
            .expect("validated display preview frame length")
    }

    pub fn encode(&self) -> Result<Bytes, PreviewFrameDecodeError> {
        validate_preview_stream_id(&self.device_id, DISPLAY_PREVIEW_ID_MAX_BYTES)?;
        let id_len = u8::try_from(self.device_id.len()).map_err(|_| {
            PreviewFrameDecodeError::PreviewIdTooLong {
                maximum: DISPLAY_PREVIEW_ID_MAX_BYTES,
                actual: self.device_id.len(),
            }
        })?;
        validate_payload(self.width, self.height, self.format, &self.payload)?;
        let encoded_len = self
            .prefix_len()
            .checked_add(self.device_id.len())
            .and_then(|len| len.checked_add(self.payload.len()))
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        let mut out = Vec::new();
        out.try_reserve_exact(encoded_len)
            .map_err(|_| PreviewFrameDecodeError::AllocationFailed { bytes: encoded_len })?;
        out.put_u8(if self.uses_compact_layout() {
            DISPLAY_PREVIEW_FRAME_TAG
        } else {
            WIDE_DISPLAY_PREVIEW_FRAME_TAG
        });
        out.put_u8(id_len);
        out.put_u32_le(self.frame_number);
        out.put_u32_le(self.timestamp_ms);
        if self.uses_compact_layout() {
            out.put_u16_le(u16::try_from(self.width).expect("compact width fits"));
            out.put_u16_le(u16::try_from(self.height).expect("compact height fits"));
        } else {
            out.put_u32_le(self.width);
            out.put_u32_le(self.height);
        }
        out.put_u8(self.format.tag());
        out.extend_from_slice(self.device_id.as_bytes());
        out.extend_from_slice(&self.payload);
        Ok(Bytes::from(out))
    }

    #[must_use]
    pub const fn uses_compact_layout(&self) -> bool {
        self.width <= u16::MAX as u32 && self.height <= u16::MAX as u32
    }

    #[must_use]
    pub const fn prefix_len(&self) -> usize {
        if self.uses_compact_layout() {
            DISPLAY_PREVIEW_FRAME_PREFIX_LEN
        } else {
            WIDE_DISPLAY_PREVIEW_FRAME_PREFIX_LEN
        }
    }

    pub fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        let header = IdentityFrameHeader::decode(input, DISPLAY_PREVIEW_LAYOUT)?;
        let end = header.end_offset(input.len())?;
        validate_payload(
            header.width,
            header.height,
            header.format,
            &input[header.payload_offset..end],
        )?;
        Ok(Self {
            device_id: header.identity,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: header.width,
            height: header.height,
            format: header.format,
            payload: Bytes::copy_from_slice(&input[header.payload_offset..end]),
        })
    }

    pub fn decode_bytes(input: &Bytes) -> Result<Self, PreviewFrameDecodeError> {
        let header = IdentityFrameHeader::decode(input, DISPLAY_PREVIEW_LAYOUT)?;
        let end = header.end_offset(input.len())?;
        validate_payload(
            header.width,
            header.height,
            header.format,
            &input[header.payload_offset..end],
        )?;
        Ok(Self {
            device_id: header.identity,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: header.width,
            height: header.height,
            format: header.format,
            payload: input.slice(header.payload_offset..end),
        })
    }
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
        validate_payload(self.width, self.height, self.format, &self.payload)?;
        let encoded_len = self
            .header_len()
            .checked_add(self.payload.len())
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        let mut out = Vec::new();
        out.try_reserve_exact(encoded_len)
            .map_err(|_| PreviewFrameDecodeError::AllocationFailed { bytes: encoded_len })?;
        if self.uses_compact_layout() {
            out.put_u8(self.channel.tag());
        } else {
            out.put_u8(WIDE_PREVIEW_FRAME_TAG);
            out.put_u8(self.channel.tag());
        }
        out.put_u32_le(self.frame_number);
        out.put_u32_le(self.timestamp_ms);
        if self.uses_compact_layout() {
            out.put_u16_le(u16::try_from(self.width).expect("compact width fits"));
            out.put_u16_le(u16::try_from(self.height).expect("compact height fits"));
        } else {
            out.put_u32_le(self.width);
            out.put_u32_le(self.height);
        }
        out.put_u8(self.format.tag());
        out.extend_from_slice(&self.payload);
        Ok(Bytes::from(out))
    }

    #[must_use]
    pub const fn uses_compact_layout(&self) -> bool {
        self.width <= u16::MAX as u32 && self.height <= u16::MAX as u32
    }

    #[must_use]
    pub const fn header_len(&self) -> usize {
        if self.uses_compact_layout() {
            PREVIEW_FRAME_HEADER_LEN
        } else {
            WIDE_PREVIEW_FRAME_HEADER_LEN
        }
    }

    pub fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        let header = PreviewFrameHeader::decode(input)?;
        let end = header.end_offset(input.len())?;
        validate_payload(
            header.width,
            header.height,
            header.format,
            &input[header.payload_offset..end],
        )?;

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
        validate_payload(
            header.width,
            header.height,
            header.format,
            &input[header.payload_offset..end],
        )?;

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
        validate_payload(self.width, self.height, self.format, &self.payload)?;
        let encoded_len = self
            .header_len()
            .checked_add(self.payload.len())
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        let mut out = Vec::new();
        out.try_reserve_exact(encoded_len)
            .map_err(|_| PreviewFrameDecodeError::AllocationFailed { bytes: encoded_len })?;
        out.put_u8(if self.uses_compact_layout() {
            ZONE_PREVIEW_FRAME_TAG
        } else {
            WIDE_ZONE_PREVIEW_FRAME_TAG
        });
        out.put_u32_le(self.frame_number);
        out.put_u32_le(self.timestamp_ms);
        out.extend_from_slice(&self.scene_id);
        out.extend_from_slice(&self.zone_id);
        if self.uses_compact_layout() {
            out.put_u16_le(u16::try_from(self.width).expect("compact width fits"));
            out.put_u16_le(u16::try_from(self.height).expect("compact height fits"));
        } else {
            out.put_u32_le(self.width);
            out.put_u32_le(self.height);
        }
        out.put_u8(self.format.tag());
        out.extend_from_slice(&self.payload);
        Ok(Bytes::from(out))
    }

    #[must_use]
    pub const fn uses_compact_layout(&self) -> bool {
        self.width <= u16::MAX as u32 && self.height <= u16::MAX as u32
    }

    #[must_use]
    pub const fn header_len(&self) -> usize {
        if self.uses_compact_layout() {
            ZONE_PREVIEW_FRAME_HEADER_LEN
        } else {
            WIDE_ZONE_PREVIEW_FRAME_HEADER_LEN
        }
    }

    pub fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        let header = ZonePreviewFrameHeader::decode(input)?;
        let end = header.end_offset(input.len())?;
        validate_payload(
            header.width,
            header.height,
            header.format,
            &input[header.payload_offset..end],
        )?;

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
        validate_payload(
            header.width,
            header.height,
            header.format,
            &input[header.payload_offset..end],
        )?;

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
        validate_preview_stream_id(&self.preview_id, INTERACTIVE_PREVIEW_ID_MAX_BYTES)?;
        let id_len = u8::try_from(self.preview_id.len()).map_err(|_| {
            PreviewFrameDecodeError::PreviewIdTooLong {
                maximum: INTERACTIVE_PREVIEW_ID_MAX_BYTES,
                actual: self.preview_id.len(),
            }
        })?;
        validate_payload(self.width, self.height, self.format, &self.payload)?;
        let encoded_len = self
            .prefix_len()
            .checked_add(self.preview_id.len())
            .and_then(|len| len.checked_add(self.payload.len()))
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        let mut out = Vec::new();
        out.try_reserve_exact(encoded_len)
            .map_err(|_| PreviewFrameDecodeError::AllocationFailed { bytes: encoded_len })?;
        out.put_u8(if self.uses_compact_layout() {
            INTERACTIVE_PREVIEW_FRAME_TAG
        } else {
            WIDE_INTERACTIVE_PREVIEW_FRAME_TAG
        });
        out.put_u8(id_len);
        out.put_u32_le(self.frame_number);
        out.put_u32_le(self.timestamp_ms);
        if self.uses_compact_layout() {
            out.put_u16_le(u16::try_from(self.width).expect("compact width fits"));
            out.put_u16_le(u16::try_from(self.height).expect("compact height fits"));
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
    pub const fn uses_compact_layout(&self) -> bool {
        self.width <= u16::MAX as u32 && self.height <= u16::MAX as u32
    }

    #[must_use]
    pub const fn prefix_len(&self) -> usize {
        if self.uses_compact_layout() {
            INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN
        } else {
            WIDE_INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN
        }
    }

    pub fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        let header = IdentityFrameHeader::decode(input, INTERACTIVE_PREVIEW_LAYOUT)?;
        let end = header.end_offset(input.len())?;
        validate_payload(
            header.width,
            header.height,
            header.format,
            &input[header.payload_offset..end],
        )?;
        Ok(Self {
            preview_id: header.identity,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: header.width,
            height: header.height,
            format: header.format,
            payload: Bytes::copy_from_slice(&input[header.payload_offset..end]),
        })
    }

    pub fn decode_bytes(input: &Bytes) -> Result<Self, PreviewFrameDecodeError> {
        let header = IdentityFrameHeader::decode(input, INTERACTIVE_PREVIEW_LAYOUT)?;
        let end = header.end_offset(input.len())?;
        validate_payload(
            header.width,
            header.height,
            header.format,
            &input[header.payload_offset..end],
        )?;
        Ok(Self {
            preview_id: header.identity,
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
/// Legacy layout:
/// tag u8 | frame u32 | timestamp u32 | source_w u16 | source_h u16 |
/// cols u8 | rows u8 | letterbox top/bottom/left/right u8 each | RGB payload
///
/// Wide-source layout uses u32 source dimensions with u8 grid and letterbox
/// dimensions. Extended layout uses u32 for every dimension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenZonesFrame {
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub grid_cols: u32,
    pub grid_rows: u32,
    /// Letterbox bars in grid units: top, bottom, left, right.
    pub letterbox: [u32; 4],
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
        let payload_len = usize::try_from(self.grid_cols)
            .ok()
            .and_then(|cols| {
                usize::try_from(self.grid_rows)
                    .ok()
                    .and_then(|rows| cols.checked_mul(rows))
            })
            .and_then(|zones| zones.checked_mul(3))
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        if self.payload.len() != payload_len {
            return Err(PreviewFrameDecodeError::PayloadLengthMismatch {
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
        let tag = if self.uses_compact_layout() {
            SCREEN_ZONES_FRAME_TAG
        } else if self.uses_compact_grid_layout() {
            WIDE_SCREEN_ZONES_FRAME_TAG
        } else {
            EXTENDED_SCREEN_ZONES_FRAME_TAG
        };
        out.put_u8(tag);
        out.put_u32_le(self.frame_number);
        out.put_u32_le(self.timestamp_ms);
        if self.uses_compact_layout() {
            out.put_u16_le(u16::try_from(self.source_width).expect("compact width fits"));
            out.put_u16_le(u16::try_from(self.source_height).expect("compact height fits"));
            out.put_u8(u8::try_from(self.grid_cols).expect("compact columns fit"));
            out.put_u8(u8::try_from(self.grid_rows).expect("compact rows fit"));
            for bar in self.letterbox {
                out.put_u8(u8::try_from(bar).expect("compact letterbox value fits"));
            }
        } else if self.uses_compact_grid_layout() {
            out.put_u32_le(self.source_width);
            out.put_u32_le(self.source_height);
            out.put_u8(u8::try_from(self.grid_cols).expect("wide columns fit"));
            out.put_u8(u8::try_from(self.grid_rows).expect("wide rows fit"));
            for bar in self.letterbox {
                out.put_u8(u8::try_from(bar).expect("wide letterbox value fits"));
            }
        } else {
            out.put_u32_le(self.source_width);
            out.put_u32_le(self.source_height);
            out.put_u32_le(self.grid_cols);
            out.put_u32_le(self.grid_rows);
            for bar in self.letterbox {
                out.put_u32_le(bar);
            }
        }
        out.extend_from_slice(&self.payload);
        Ok(Bytes::from(out))
    }

    #[must_use]
    pub const fn uses_compact_layout(&self) -> bool {
        self.source_width <= u16::MAX as u32
            && self.source_height <= u16::MAX as u32
            && self.uses_compact_grid_layout()
    }

    #[must_use]
    pub const fn uses_compact_grid_layout(&self) -> bool {
        self.grid_cols <= u8::MAX as u32
            && self.grid_rows <= u8::MAX as u32
            && self.letterbox[0] <= u8::MAX as u32
            && self.letterbox[1] <= u8::MAX as u32
            && self.letterbox[2] <= u8::MAX as u32
            && self.letterbox[3] <= u8::MAX as u32
    }

    #[must_use]
    pub const fn header_len(&self) -> usize {
        if self.uses_compact_layout() {
            SCREEN_ZONES_FRAME_HEADER_LEN
        } else if self.uses_compact_grid_layout() {
            WIDE_SCREEN_ZONES_FRAME_HEADER_LEN
        } else {
            EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN
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
    pub fn zone_rgb(&self, row: u32, col: u32) -> Option<[u8; 3]> {
        if row >= self.grid_rows || col >= self.grid_cols {
            return None;
        }
        let offset = usize::try_from(row)
            .ok()?
            .checked_mul(usize::try_from(self.grid_cols).ok()?)?
            .checked_add(usize::try_from(col).ok()?)?
            .checked_mul(3)?;
        let bytes = self.payload.get(offset..offset + 3)?;
        Some([bytes[0], bytes[1], bytes[2]])
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum PreviewStreamId {
    Passive(PreviewFrameChannel),
    Zone {
        scene_id: [u8; 16],
        zone_id: [u8; 16],
    },
    Interactive(String),
    /// One keyed `display_preview` subscription, identified by device.
    Display(String),
    ScreenZones,
}

impl PreviewStreamId {
    #[must_use]
    pub fn identity_bytes(&self) -> usize {
        match self {
            Self::Passive(_) | Self::ScreenZones => 0,
            Self::Zone { .. } => 32,
            Self::Interactive(preview_id) => preview_id.len(),
            Self::Display(device_id) => device_id.len(),
        }
    }

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
                validate_preview_stream_id(preview_id, INTERACTIVE_PREVIEW_ID_MAX_BYTES)?;
                Ok((
                    2,
                    INTERACTIVE_PREVIEW_FRAME_TAG,
                    preview_id.as_bytes().to_vec(),
                ))
            }
            Self::ScreenZones => Ok((3, SCREEN_ZONES_FRAME_TAG, Vec::new())),
            Self::Display(device_id) => {
                validate_preview_stream_id(device_id, DISPLAY_PREVIEW_ID_MAX_BYTES)?;
                Ok((4, DISPLAY_PREVIEW_FRAME_TAG, device_id.as_bytes().to_vec()))
            }
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
            2 if channel == INTERACTIVE_PREVIEW_FRAME_TAG => Ok(Self::Interactive(
                decode_stream_identity(identity, INTERACTIVE_PREVIEW_ID_MAX_BYTES)?,
            )),
            3 if channel == SCREEN_ZONES_FRAME_TAG && identity.is_empty() => Ok(Self::ScreenZones),
            4 if channel == DISPLAY_PREVIEW_FRAME_TAG => Ok(Self::Display(decode_stream_identity(
                identity,
                DISPLAY_PREVIEW_ID_MAX_BYTES,
            )?)),
            _ => Err(PreviewChunkError::InvalidStreamIdentity),
        }
    }
}

fn decode_stream_identity(identity: &[u8], maximum: usize) -> Result<String, PreviewChunkError> {
    let decoded = std::str::from_utf8(identity)
        .map_err(|_| PreviewChunkError::Frame(PreviewFrameDecodeError::InvalidPreviewIdUtf8))?
        .to_owned();
    validate_preview_stream_id(&decoded, maximum).map_err(PreviewChunkError::Frame)?;
    Ok(decoded)
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
        if u64::from(self.chunk_count) > self.total_encoded_bytes {
            return Err(PreviewChunkError::ImpossibleChunkCount {
                chunks: self.chunk_count,
                total_bytes: self.total_encoded_bytes,
            });
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewCancelFrame {
    pub stream: PreviewStreamId,
    pub publication_id: u64,
}

impl PreviewCancelFrame {
    pub fn try_encode(&self) -> Result<Bytes, PreviewChunkError> {
        let (stream_kind, channel, identity) =
            self.stream.wire_parts().map_err(PreviewChunkError::Frame)?;
        let identity_len =
            u16::try_from(identity.len()).map_err(|_| PreviewChunkError::InvalidStreamIdentity)?;
        let encoded_len = PREVIEW_CANCEL_FIXED_HEADER_LEN
            .checked_add(identity.len())
            .ok_or(PreviewChunkError::LengthOverflow)?;
        let mut out = Vec::new();
        out.try_reserve_exact(encoded_len)
            .map_err(|_| PreviewChunkError::AllocationFailed { bytes: encoded_len })?;
        out.put_u8(PREVIEW_CANCEL_FRAME_TAG);
        out.put_u8(PREVIEW_CANCEL_SCHEMA);
        out.put_u8(stream_kind);
        out.put_u8(channel);
        out.put_u16_le(identity_len);
        out.put_u64_le(self.publication_id);
        out.extend_from_slice(&identity);
        Ok(Bytes::from(out))
    }

    pub fn decode_bytes(input: &Bytes) -> Result<Self, PreviewChunkError> {
        if input.len() < PREVIEW_CANCEL_FIXED_HEADER_LEN {
            return Err(PreviewChunkError::TooShort {
                expected: PREVIEW_CANCEL_FIXED_HEADER_LEN,
                actual: input.len(),
            });
        }
        if input[0] != PREVIEW_CANCEL_FRAME_TAG {
            return Err(PreviewChunkError::UnknownTag { actual: input[0] });
        }
        if input[1] != PREVIEW_CANCEL_SCHEMA {
            return Err(PreviewChunkError::UnknownSchema { actual: input[1] });
        }
        let identity_len = usize::from(u16::from_le_bytes(
            input[4..6].try_into().expect("slice has 2 bytes"),
        ));
        let expected = PREVIEW_CANCEL_FIXED_HEADER_LEN
            .checked_add(identity_len)
            .ok_or(PreviewChunkError::LengthOverflow)?;
        if input.len() != expected {
            return Err(PreviewChunkError::MessageLengthMismatch {
                expected,
                actual: input.len(),
            });
        }
        Ok(Self {
            stream: PreviewStreamId::decode(
                input[2],
                input[3],
                &input[PREVIEW_CANCEL_FIXED_HEADER_LEN..],
            )?,
            publication_id: u64::from_le_bytes(input[6..14].try_into().expect("slice has 8 bytes")),
        })
    }
}

pub fn split_preview_publication(
    encoded: &Bytes,
    metadata: &PreviewPublicationMetadata,
    max_message_bytes: usize,
) -> Result<Vec<Bytes>, PreviewChunkError> {
    split_preview_publication_with_limits(
        encoded,
        metadata,
        max_message_bytes,
        PreviewTransportLimits::default(),
    )
}

pub fn split_preview_publication_with_limits(
    encoded: &Bytes,
    metadata: &PreviewPublicationMetadata,
    max_message_bytes: usize,
    limits: PreviewTransportLimits,
) -> Result<Vec<Bytes>, PreviewChunkError> {
    if encoded.len() > limits.max_encoded_publication_bytes {
        return Err(PreviewChunkError::PublicationBudgetExceeded {
            requested: encoded.len(),
            limit: limits.max_encoded_publication_bytes,
        });
    }
    if max_message_bytes > limits.max_message_bytes {
        return Err(PreviewChunkError::MessageBudgetExceeded {
            requested: max_message_bytes,
            limit: limits.max_message_bytes,
        });
    }
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
    let max_chunk_count = limits.effective_max_chunk_count(identity.len());
    if chunk_count_u32 > max_chunk_count {
        return Err(PreviewChunkError::ChunkCountBudgetExceeded {
            requested: chunk_count_u32,
            limit: max_chunk_count,
        });
    }
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
    pub max_decoded_publication_bytes: usize,
    pub max_encoded_publication_bytes: usize,
    pub max_connection_bytes: usize,
    pub max_idle_ms: u64,
    pub max_message_bytes: usize,
    pub max_reassembly_state_bytes: usize,
    pub max_tombstone_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewTransportLimits {
    pub max_decoded_publication_bytes: usize,
    pub max_encoded_publication_bytes: usize,
    pub max_connection_bytes: usize,
    pub max_idle_ms: u64,
    pub max_message_bytes: usize,
    pub max_reassembly_state_bytes: usize,
    pub max_tombstone_bytes: usize,
    pub max_sender_state_bytes: usize,
    pub max_cursor_state_bytes: usize,
}

impl PreviewTransportLimits {
    #[must_use]
    pub fn effective_max_chunk_count(self, _identity_bytes: usize) -> u32 {
        u32::try_from(self.max_encoded_publication_bytes).unwrap_or(u32::MAX)
    }
}

impl Default for PreviewTransportLimits {
    fn default() -> Self {
        Self {
            max_decoded_publication_bytes: DEFAULT_PREVIEW_MAX_DECODED_PUBLICATION_BYTES,
            max_encoded_publication_bytes: DEFAULT_PREVIEW_MAX_ENCODED_PUBLICATION_BYTES,
            max_connection_bytes: DEFAULT_PREVIEW_MAX_CONNECTION_BYTES,
            max_idle_ms: DEFAULT_PREVIEW_MAX_IDLE_MS,
            max_message_bytes: DEFAULT_PREVIEW_MAX_MESSAGE_BYTES,
            max_reassembly_state_bytes: DEFAULT_PREVIEW_MAX_REASSEMBLY_STATE_BYTES,
            max_tombstone_bytes: DEFAULT_PREVIEW_MAX_TOMBSTONE_BYTES,
            max_sender_state_bytes: DEFAULT_PREVIEW_MAX_SENDER_STATE_BYTES,
            max_cursor_state_bytes: DEFAULT_PREVIEW_MAX_CURSOR_STATE_BYTES,
        }
    }
}

impl PreviewReassemblyLimits {
    #[must_use]
    pub fn effective_max_chunk_count(self, identity_bytes: usize) -> u32 {
        PreviewTransportLimits {
            max_decoded_publication_bytes: self.max_decoded_publication_bytes,
            max_encoded_publication_bytes: self.max_encoded_publication_bytes,
            max_connection_bytes: self.max_connection_bytes,
            max_idle_ms: self.max_idle_ms,
            max_message_bytes: self.max_message_bytes,
            max_reassembly_state_bytes: self.max_reassembly_state_bytes,
            max_tombstone_bytes: self.max_tombstone_bytes,
            max_sender_state_bytes: usize::MAX,
            max_cursor_state_bytes: usize::MAX,
        }
        .effective_max_chunk_count(identity_bytes)
    }
}

impl Default for PreviewReassemblyLimits {
    fn default() -> Self {
        let limits = PreviewTransportLimits::default();
        Self {
            max_decoded_publication_bytes: limits.max_decoded_publication_bytes,
            max_encoded_publication_bytes: limits.max_encoded_publication_bytes,
            max_connection_bytes: limits.max_connection_bytes,
            max_idle_ms: limits.max_idle_ms,
            max_message_bytes: limits.max_message_bytes,
            max_reassembly_state_bytes: limits.max_reassembly_state_bytes,
            max_tombstone_bytes: limits.max_tombstone_bytes,
        }
    }
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
    screen_zones_header_validated: bool,
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct PreviewStreamState {
    high_water_publication_id: u64,
    partial: Option<PartialPreviewPublication>,
    last_activity_ms: u64,
    tombstone_generation: u64,
}

#[derive(Debug)]
pub struct PreviewChunkReassembler {
    limits: PreviewReassemblyLimits,
    streams: HashMap<Arc<PreviewStreamId>, PreviewStreamState>,
    active_streams: usize,
    tombstones: usize,
    reserved_bytes: usize,
    state_bytes: usize,
    tombstone_bytes: usize,
    logical_now_ms: u64,
    next_tombstone_generation: u64,
    superseded_publications: u64,
    expired_publications: u64,
}

impl PreviewChunkReassembler {
    #[must_use]
    pub fn new(limits: PreviewReassemblyLimits) -> Self {
        Self {
            limits,
            streams: HashMap::new(),
            active_streams: 0,
            tombstones: 0,
            reserved_bytes: 0,
            state_bytes: 0,
            tombstone_bytes: 0,
            logical_now_ms: 0,
            next_tombstone_generation: 1,
            superseded_publications: 0,
            expired_publications: 0,
        }
    }

    pub fn push(
        &mut self,
        encoded_chunk: &Bytes,
    ) -> Result<Option<ReassembledPreviewPublication>, PreviewChunkError> {
        self.logical_now_ms = self.logical_now_ms.saturating_add(1);
        self.push_at(encoded_chunk, self.logical_now_ms)
    }

    pub fn push_at(
        &mut self,
        encoded_chunk: &Bytes,
        now_ms: u64,
    ) -> Result<Option<ReassembledPreviewPublication>, PreviewChunkError> {
        self.logical_now_ms = self.logical_now_ms.max(now_ms);
        self.expire_at(now_ms);
        if encoded_chunk.len() > self.limits.max_message_bytes {
            return Err(PreviewChunkError::MessageBudgetExceeded {
                requested: encoded_chunk.len(),
                limit: self.limits.max_message_bytes,
            });
        }
        let chunk = PreviewChunkFrame::decode_bytes(encoded_chunk)?;
        let max_chunk_count = self
            .limits
            .effective_max_chunk_count(chunk.metadata.stream.identity_bytes());
        if chunk.chunk_count > max_chunk_count {
            return Err(PreviewChunkError::ChunkCountBudgetExceeded {
                requested: chunk.chunk_count,
                limit: max_chunk_count,
            });
        }
        let total_encoded_bytes = usize::try_from(chunk.total_encoded_bytes)
            .map_err(|_| PreviewChunkError::LengthOverflow)?;
        if total_encoded_bytes > self.limits.max_encoded_publication_bytes {
            return Err(PreviewChunkError::PublicationBudgetExceeded {
                requested: total_encoded_bytes,
                limit: self.limits.max_encoded_publication_bytes,
            });
        }

        let stream = chunk.metadata.stream.clone();
        let publication_id = chunk.metadata.publication_id;
        if let Some(state) = self.streams.get(&stream)
            && (publication_id < state.high_water_publication_id
                || (publication_id == state.high_water_publication_id && state.partial.is_none()))
        {
            return Err(PreviewChunkError::StalePublication {
                publication_id,
                high_water: state.high_water_publication_id,
            });
        }

        let starts_new_publication = self
            .streams
            .get(&stream)
            .is_none_or(|state| publication_id > state.high_water_publication_id);
        if starts_new_publication {
            self.admit_partial(&chunk, total_encoded_bytes, now_ms)?;
        }
        self.append_chunk(&stream, &chunk, total_encoded_bytes, now_ms)
    }

    fn admit_partial(
        &mut self,
        chunk: &PreviewChunkFrame,
        total_encoded_bytes: usize,
        now_ms: u64,
    ) -> Result<(), PreviewChunkError> {
        if chunk.chunk_index != 0 || chunk.chunk_offset != 0 {
            return Err(PreviewChunkError::MissingPublicationStart);
        }
        let screen_zones_header_validated =
            validate_reassembly_admission(chunk, total_encoded_bytes, self.limits)?;
        let stream = &chunk.metadata.stream;
        let existing_is_active = self
            .streams
            .get(stream)
            .is_some_and(|state| state.partial.is_some());
        let stream_state_bytes = preview_stream_state_bytes(stream)?;
        let existing_is_tombstone = self
            .streams
            .get(stream)
            .is_some_and(|state| state.partial.is_none());
        let requested_state_bytes = self
            .state_bytes
            .checked_add(usize::from(!existing_is_active) * stream_state_bytes)
            .ok_or(PreviewChunkError::LengthOverflow)?;
        if requested_state_bytes > self.limits.max_reassembly_state_bytes {
            return Err(PreviewChunkError::ReassemblyStateBudgetExceeded {
                requested: requested_state_bytes,
                limit: self.limits.max_reassembly_state_bytes,
            });
        }

        let replaced_bytes = self
            .streams
            .get(stream)
            .and_then(|state| state.partial.as_ref())
            .map_or(0, |partial| partial.total_encoded_bytes);
        let retained_bytes = self
            .reserved_bytes
            .checked_sub(replaced_bytes)
            .ok_or(PreviewChunkError::LengthOverflow)?;
        let requested = retained_bytes
            .checked_add(total_encoded_bytes)
            .ok_or(PreviewChunkError::LengthOverflow)?;
        if requested > self.limits.max_connection_bytes {
            return Err(PreviewChunkError::ConnectionBudgetExceeded {
                requested,
                limit: self.limits.max_connection_bytes,
            });
        }
        let initial_reservation = if screen_zones_header_validated {
            total_encoded_bytes
        } else {
            total_encoded_bytes.min(EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN)
        };
        let replaced = self.streams.get_mut(stream).and_then(|state| {
            state.high_water_publication_id = chunk.metadata.publication_id;
            state.partial.take()
        });
        if let Some(replaced) = &replaced {
            self.reserved_bytes = self
                .reserved_bytes
                .saturating_sub(replaced.total_encoded_bytes);
        }
        drop(replaced);

        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(initial_reservation).is_err() {
            if existing_is_active {
                self.active_streams = self.active_streams.saturating_sub(1);
                self.mark_tombstone(stream, true);
            }
            return Err(PreviewChunkError::AllocationFailed {
                bytes: initial_reservation,
            });
        }
        let partial = PartialPreviewPublication {
            metadata: chunk.metadata.clone(),
            total_encoded_bytes,
            chunk_count: chunk.chunk_count,
            next_chunk_index: 0,
            screen_zones_header_validated,
            bytes,
        };
        if !self.streams.contains_key(stream) {
            self.streams
                .try_reserve(1)
                .map_err(|_| PreviewChunkError::AllocationFailed { bytes: 0 })?;
        }
        if existing_is_tombstone {
            self.tombstones = self.tombstones.saturating_sub(1);
            self.tombstone_bytes = self.tombstone_bytes.saturating_sub(stream_state_bytes);
        }
        let stream_handle = self.streams.get_key_value(stream).map_or_else(
            || Arc::new(stream.clone()),
            |(handle, _)| Arc::clone(handle),
        );
        self.streams.insert(
            stream_handle,
            PreviewStreamState {
                high_water_publication_id: chunk.metadata.publication_id,
                partial: Some(partial),
                last_activity_ms: now_ms,
                tombstone_generation: 0,
            },
        );
        if existing_is_active {
            self.superseded_publications = self.superseded_publications.saturating_add(1);
        }
        if !existing_is_active {
            self.active_streams = self.active_streams.saturating_add(1);
            self.state_bytes = requested_state_bytes;
        }
        self.reserved_bytes = requested;
        Ok(())
    }

    fn append_chunk(
        &mut self,
        stream: &PreviewStreamId,
        chunk: &PreviewChunkFrame,
        total_encoded_bytes: usize,
        now_ms: u64,
    ) -> Result<Option<ReassembledPreviewPublication>, PreviewChunkError> {
        let state = self
            .streams
            .get_mut(stream)
            .ok_or(PreviewChunkError::MissingPublicationStart)?;
        let partial = state
            .partial
            .as_mut()
            .ok_or(PreviewChunkError::DuplicateChunk)?;
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
        if !partial.screen_zones_header_validated {
            let prefix_len = partial
                .bytes
                .len()
                .checked_add(chunk.payload.len())
                .ok_or(PreviewChunkError::LengthOverflow)?
                .min(EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN);
            let mut prefix = [0_u8; EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN];
            let retained_prefix_len = partial.bytes.len().min(prefix_len);
            prefix[..retained_prefix_len].copy_from_slice(&partial.bytes[..retained_prefix_len]);
            let incoming_prefix_len = prefix_len.saturating_sub(retained_prefix_len);
            prefix[retained_prefix_len..prefix_len]
                .copy_from_slice(&chunk.payload[..incoming_prefix_len]);
            if validate_screen_zones_header_prefix(
                &partial.metadata,
                partial.total_encoded_bytes,
                &prefix[..prefix_len],
                self.limits,
            )? {
                let additional = partial
                    .total_encoded_bytes
                    .checked_sub(partial.bytes.len())
                    .ok_or(PreviewChunkError::LengthOverflow)?;
                partial.bytes.try_reserve_exact(additional).map_err(|_| {
                    PreviewChunkError::AllocationFailed {
                        bytes: partial.total_encoded_bytes,
                    }
                })?;
                partial.screen_zones_header_validated = true;
            }
        }
        partial.bytes.extend_from_slice(&chunk.payload);
        partial.next_chunk_index = partial
            .next_chunk_index
            .checked_add(1)
            .ok_or(PreviewChunkError::LengthOverflow)?;
        state.last_activity_ms = now_ms;

        if partial.next_chunk_index != partial.chunk_count {
            return Ok(None);
        }
        if partial.bytes.len() != partial.total_encoded_bytes {
            return Err(PreviewChunkError::NonContiguousChunk);
        }
        let completed = state
            .partial
            .take()
            .ok_or(PreviewChunkError::DuplicateChunk)?;
        self.reserved_bytes = self
            .reserved_bytes
            .checked_sub(completed.total_encoded_bytes)
            .ok_or(PreviewChunkError::LengthOverflow)?;
        self.active_streams = self.active_streams.saturating_sub(1);
        self.mark_tombstone(stream, true);
        let encoded = Bytes::from(completed.bytes);
        validate_completed_publication(&completed.metadata, &encoded, self.limits)?;
        Ok(Some(ReassembledPreviewPublication {
            metadata: completed.metadata,
            encoded,
        }))
    }

    pub fn expire_at(&mut self, now_ms: u64) -> usize {
        let max_idle_ms = self.limits.max_idle_ms;
        let mut expired_bytes = 0_usize;
        let mut expired_state_bytes = 0_usize;
        let mut expired_publications = 0_u64;
        for (stream, state) in &mut self.streams {
            let expired = state.partial.is_some()
                && now_ms.saturating_sub(state.last_activity_ms) >= max_idle_ms;
            if !expired {
                continue;
            }
            if let Some(partial) = state.partial.take() {
                expired_bytes = expired_bytes.saturating_add(partial.total_encoded_bytes);
                expired_state_bytes = expired_state_bytes
                    .saturating_add(preview_stream_state_bytes(stream).unwrap_or(usize::MAX));
                expired_publications = expired_publications.saturating_add(1);
            }
            state.tombstone_generation = self.next_tombstone_generation;
            self.next_tombstone_generation = self.next_tombstone_generation.saturating_add(1);
            self.tombstones = self.tombstones.saturating_add(1);
        }
        self.active_streams = self
            .active_streams
            .saturating_sub(usize::try_from(expired_publications).unwrap_or(usize::MAX));
        self.reserved_bytes = self.reserved_bytes.saturating_sub(expired_bytes);
        self.state_bytes = self.state_bytes.saturating_sub(expired_state_bytes);
        self.tombstone_bytes = self.tombstone_bytes.saturating_add(expired_state_bytes);
        self.expired_publications = self
            .expired_publications
            .saturating_add(expired_publications);
        self.prune_tombstones();
        usize::try_from(expired_publications).unwrap_or(usize::MAX)
    }

    #[must_use]
    pub fn next_expiry_ms(&self) -> Option<u64> {
        self.streams
            .values()
            .filter(|state| state.partial.is_some())
            .map(|state| {
                state
                    .last_activity_ms
                    .saturating_add(self.limits.max_idle_ms)
            })
            .min()
    }

    #[must_use]
    pub const fn superseded_publications(&self) -> u64 {
        self.superseded_publications
    }

    #[must_use]
    pub const fn expired_publications(&self) -> u64 {
        self.expired_publications
    }

    #[must_use]
    pub const fn reserved_bytes(&self) -> usize {
        self.reserved_bytes
    }

    #[must_use]
    pub const fn state_bytes(&self) -> usize {
        self.state_bytes
    }

    #[must_use]
    pub const fn tombstone_bytes(&self) -> usize {
        self.tombstone_bytes
    }

    #[must_use]
    pub fn partial_count(&self) -> usize {
        self.active_streams
    }

    #[must_use]
    pub const fn tombstone_count(&self) -> usize {
        self.tombstones
    }

    pub fn cancel_publication(
        &mut self,
        cancellation: &PreviewCancelFrame,
    ) -> Result<bool, PreviewChunkError> {
        if !self.streams.contains_key(&cancellation.stream) {
            self.streams
                .try_reserve(1)
                .map_err(|_| PreviewChunkError::AllocationFailed { bytes: 0 })?;
            self.streams.insert(
                Arc::new(cancellation.stream.clone()),
                PreviewStreamState {
                    high_water_publication_id: cancellation.publication_id,
                    partial: None,
                    last_activity_ms: self.logical_now_ms,
                    tombstone_generation: 0,
                },
            );
            self.mark_tombstone(&cancellation.stream, false);
            return Ok(false);
        }

        let state = self
            .streams
            .get_mut(&cancellation.stream)
            .expect("stream existence checked");
        if state.high_water_publication_id > cancellation.publication_id {
            return Ok(false);
        }
        state.high_water_publication_id = cancellation.publication_id;
        let cancelled = state.partial.take();
        if let Some(partial) = &cancelled {
            self.reserved_bytes = self
                .reserved_bytes
                .saturating_sub(partial.total_encoded_bytes);
            self.active_streams = self.active_streams.saturating_sub(1);
        }
        self.mark_tombstone(&cancellation.stream, cancelled.is_some());
        Ok(cancelled.is_some())
    }

    pub fn clear(&mut self) {
        self.streams.clear();
        self.active_streams = 0;
        self.tombstones = 0;
        self.reserved_bytes = 0;
        self.state_bytes = 0;
        self.tombstone_bytes = 0;
    }

    fn mark_tombstone(&mut self, stream: &PreviewStreamId, was_active: bool) {
        let stream_state_bytes = preview_stream_state_bytes(stream).unwrap_or(usize::MAX);
        let is_new_tombstone = self
            .streams
            .get(stream)
            .is_some_and(|state| state.tombstone_generation == 0);
        if is_new_tombstone {
            self.make_tombstone_room(stream_state_bytes);
            if stream_state_bytes > self.limits.max_tombstone_bytes {
                if was_active {
                    self.state_bytes = self.state_bytes.saturating_sub(stream_state_bytes);
                }
                self.streams.remove(stream);
                return;
            }
        }
        let state = self
            .streams
            .get_mut(stream)
            .expect("tombstone stream must exist");
        if state.tombstone_generation == 0 {
            self.tombstones = self.tombstones.saturating_add(1);
            if was_active {
                self.state_bytes = self.state_bytes.saturating_sub(stream_state_bytes);
            }
            self.tombstone_bytes = self.tombstone_bytes.saturating_add(stream_state_bytes);
        }
        state.tombstone_generation = self.next_tombstone_generation;
        self.next_tombstone_generation = self.next_tombstone_generation.saturating_add(1);
        self.prune_tombstones();
    }

    fn prune_tombstones(&mut self) {
        while self.tombstone_bytes > self.limits.max_tombstone_bytes {
            let Some(oldest_generation) = self
                .streams
                .values()
                .filter(|state| state.partial.is_none() && state.tombstone_generation != 0)
                .map(|state| state.tombstone_generation)
                .min()
            else {
                break;
            };
            let mut removed = false;
            let mut removed_bytes = 0;
            self.streams.retain(|stream, state| {
                let evict = !removed
                    && state.partial.is_none()
                    && state.tombstone_generation != 0
                    && state.tombstone_generation == oldest_generation;
                removed |= evict;
                if evict {
                    removed_bytes = preview_stream_state_bytes(stream).unwrap_or(usize::MAX);
                }
                !evict
            });
            if removed {
                self.tombstones = self.tombstones.saturating_sub(1);
                self.tombstone_bytes = self.tombstone_bytes.saturating_sub(removed_bytes);
            } else {
                break;
            }
        }
    }

    fn make_tombstone_room(&mut self, additional_bytes: usize) {
        while self
            .tombstone_bytes
            .checked_add(additional_bytes)
            .is_none_or(|bytes| bytes > self.limits.max_tombstone_bytes)
            && self.tombstones > 0
        {
            let Some(oldest_generation) = self
                .streams
                .values()
                .filter(|state| state.partial.is_none() && state.tombstone_generation != 0)
                .map(|state| state.tombstone_generation)
                .min()
            else {
                break;
            };
            let mut removed = false;
            let mut removed_bytes = 0;
            self.streams.retain(|stream, state| {
                let evict = !removed
                    && state.partial.is_none()
                    && state.tombstone_generation == oldest_generation;
                removed |= evict;
                if evict {
                    removed_bytes = preview_stream_state_bytes(stream).unwrap_or(usize::MAX);
                }
                !evict
            });
            if !removed {
                break;
            }
            self.tombstones = self.tombstones.saturating_sub(1);
            self.tombstone_bytes = self.tombstone_bytes.saturating_sub(removed_bytes);
        }
    }
}

fn preview_stream_state_bytes(stream: &PreviewStreamId) -> Result<usize, PreviewChunkError> {
    std::mem::size_of::<(Arc<PreviewStreamId>, PreviewStreamState)>()
        .checked_add(std::mem::size_of::<(usize, usize)>())
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<PreviewStreamId>()))
        .and_then(|bytes| bytes.checked_add(stream.identity_bytes()))
        .ok_or(PreviewChunkError::LengthOverflow)
}

fn validate_reassembly_admission(
    chunk: &PreviewChunkFrame,
    total_encoded_bytes: usize,
    limits: PreviewReassemblyLimits,
) -> Result<bool, PreviewChunkError> {
    let metadata = &chunk.metadata;
    if metadata.width == 0 || metadata.height == 0 {
        return Err(PreviewChunkError::InvalidGeometry {
            width: metadata.width,
            height: metadata.height,
        });
    }

    if matches!(metadata.stream, PreviewStreamId::ScreenZones) {
        if metadata.format != PreviewPixelFormat::Rgb || total_encoded_bytes == 0 {
            return Err(PreviewChunkError::InvalidLayout);
        }
        let minimum_decoded_bytes =
            total_encoded_bytes.saturating_sub(EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN);
        if minimum_decoded_bytes > limits.max_decoded_publication_bytes {
            return Err(PreviewChunkError::DecodedPublicationBudgetExceeded {
                requested: minimum_decoded_bytes,
                limit: limits.max_decoded_publication_bytes,
            });
        }
        return validate_screen_zones_header_prefix(
            metadata,
            total_encoded_bytes,
            &chunk.payload,
            limits,
        );
    }

    let header_len = publication_header_len(metadata)?;
    let decoded_bytes =
        raw_payload_len(metadata.width, metadata.height, 4).map_err(PreviewChunkError::Frame)?;
    if decoded_bytes > limits.max_decoded_publication_bytes {
        return Err(PreviewChunkError::DecodedPublicationBudgetExceeded {
            requested: decoded_bytes,
            limit: limits.max_decoded_publication_bytes,
        });
    }

    if let Some(bytes_per_pixel) = metadata.format.bytes_per_pixel() {
        let expected = header_len
            .checked_add(
                raw_payload_len(metadata.width, metadata.height, bytes_per_pixel)
                    .map_err(PreviewChunkError::Frame)?,
            )
            .ok_or(PreviewChunkError::LengthOverflow)?;
        if total_encoded_bytes != expected {
            return Err(PreviewChunkError::RawPublicationLengthMismatch {
                expected,
                actual: total_encoded_bytes,
            });
        }
    } else if total_encoded_bytes <= header_len {
        return Err(PreviewChunkError::InvalidLayout);
    }
    Ok(true)
}

fn validate_screen_zones_header_prefix(
    metadata: &PreviewPublicationMetadata,
    total_encoded_bytes: usize,
    prefix: &[u8],
    limits: PreviewReassemblyLimits,
) -> Result<bool, PreviewChunkError> {
    let tag = prefix
        .first()
        .copied()
        .ok_or(PreviewChunkError::InvalidLayout)?;
    let header_len = match tag {
        SCREEN_ZONES_FRAME_TAG => SCREEN_ZONES_FRAME_HEADER_LEN,
        WIDE_SCREEN_ZONES_FRAME_TAG => WIDE_SCREEN_ZONES_FRAME_HEADER_LEN,
        EXTENDED_SCREEN_ZONES_FRAME_TAG => EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN,
        actual => {
            return Err(PreviewChunkError::Frame(
                PreviewFrameDecodeError::UnknownChannel { actual },
            ));
        }
    };
    if prefix.len() < header_len {
        return Ok(false);
    }
    let header =
        ScreenZonesFrameHeader::decode(&prefix[..header_len]).map_err(PreviewChunkError::Frame)?;
    let end = header
        .end_offset(total_encoded_bytes)
        .map_err(PreviewChunkError::Frame)?;
    let decoded_bytes = end
        .checked_sub(header.payload_offset)
        .ok_or(PreviewChunkError::LengthOverflow)?;
    if decoded_bytes > limits.max_decoded_publication_bytes {
        return Err(PreviewChunkError::DecodedPublicationBudgetExceeded {
            requested: decoded_bytes,
            limit: limits.max_decoded_publication_bytes,
        });
    }
    if header.frame_number != metadata.frame_number
        || header.timestamp_ms != metadata.timestamp_ms
        || header.source_width != metadata.width
        || header.source_height != metadata.height
    {
        return Err(PreviewChunkError::MetadataChanged);
    }
    Ok(true)
}

fn validate_completed_publication(
    metadata: &PreviewPublicationMetadata,
    encoded: &Bytes,
    limits: PreviewReassemblyLimits,
) -> Result<(), PreviewChunkError> {
    let metadata_matches = match &metadata.stream {
        PreviewStreamId::Passive(channel) => {
            let frame = PreviewFrame::decode_bytes(encoded).map_err(PreviewChunkError::Frame)?;
            frame.channel == *channel
                && frame.frame_number == metadata.frame_number
                && frame.timestamp_ms == metadata.timestamp_ms
                && frame.width == metadata.width
                && frame.height == metadata.height
                && frame.format == metadata.format
        }
        PreviewStreamId::Zone { scene_id, zone_id } => {
            let frame =
                ZonePreviewFrame::decode_bytes(encoded).map_err(PreviewChunkError::Frame)?;
            frame.scene_id == *scene_id
                && frame.zone_id == *zone_id
                && frame.frame_number == metadata.frame_number
                && frame.timestamp_ms == metadata.timestamp_ms
                && frame.width == metadata.width
                && frame.height == metadata.height
                && frame.format == metadata.format
        }
        PreviewStreamId::Interactive(preview_id) => {
            let frame =
                InteractivePreviewFrame::decode_bytes(encoded).map_err(PreviewChunkError::Frame)?;
            frame.preview_id == *preview_id
                && frame.frame_number == metadata.frame_number
                && frame.timestamp_ms == metadata.timestamp_ms
                && frame.width == metadata.width
                && frame.height == metadata.height
                && frame.format == metadata.format
        }
        PreviewStreamId::Display(device_id) => {
            let frame =
                DisplayPreviewFrame::decode_bytes(encoded).map_err(PreviewChunkError::Frame)?;
            frame.device_id == *device_id
                && frame.frame_number == metadata.frame_number
                && frame.timestamp_ms == metadata.timestamp_ms
                && frame.width == metadata.width
                && frame.height == metadata.height
                && frame.format == metadata.format
        }
        PreviewStreamId::ScreenZones => {
            validate_screen_zones_header_prefix(metadata, encoded.len(), encoded, limits)?
        }
    };
    if metadata_matches {
        Ok(())
    } else {
        Err(PreviewChunkError::MetadataChanged)
    }
}

fn publication_header_len(
    metadata: &PreviewPublicationMetadata,
) -> Result<usize, PreviewChunkError> {
    let wide = metadata.width > u32::from(u16::MAX) || metadata.height > u32::from(u16::MAX);
    match &metadata.stream {
        PreviewStreamId::Passive(_) => Ok(if wide {
            WIDE_PREVIEW_FRAME_HEADER_LEN
        } else {
            PREVIEW_FRAME_HEADER_LEN
        }),
        PreviewStreamId::Zone { .. } => Ok(if wide {
            WIDE_ZONE_PREVIEW_FRAME_HEADER_LEN
        } else {
            ZONE_PREVIEW_FRAME_HEADER_LEN
        }),
        PreviewStreamId::Interactive(preview_id) => {
            identity_publication_header_len(preview_id, INTERACTIVE_PREVIEW_LAYOUT, wide)
        }
        PreviewStreamId::Display(device_id) => {
            identity_publication_header_len(device_id, DISPLAY_PREVIEW_LAYOUT, wide)
        }
        PreviewStreamId::ScreenZones => {
            if metadata.format != PreviewPixelFormat::Rgb {
                return Err(PreviewChunkError::MetadataChanged);
            }
            Ok(if wide {
                WIDE_SCREEN_ZONES_FRAME_HEADER_LEN
            } else {
                SCREEN_ZONES_FRAME_HEADER_LEN
            })
        }
    }
}

fn identity_publication_header_len(
    identity: &str,
    layout: IdentityFrameLayout,
    wide: bool,
) -> Result<usize, PreviewChunkError> {
    validate_preview_stream_id(identity, layout.max_identity_bytes)
        .map_err(PreviewChunkError::Frame)?;
    (if wide {
        layout.wide_prefix_len
    } else {
        layout.prefix_len
    })
    .checked_add(identity.len())
    .ok_or(PreviewChunkError::LengthOverflow)
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
    #[error("preview WebSocket message needs {requested} bytes; limit is {limit}")]
    MessageBudgetExceeded { requested: usize, limit: usize },
    #[error("preview publication needs {requested} chunks; limit is {limit}")]
    ChunkCountBudgetExceeded { requested: u32, limit: u32 },
    #[error("preview control message must be {expected} bytes, got {actual}")]
    MessageLengthMismatch { expected: usize, actual: usize },
    #[error("preview publication needs {requested} bytes; limit is {limit}")]
    PublicationBudgetExceeded { requested: usize, limit: usize },
    #[error("decoded preview publication needs {requested} bytes; limit is {limit}")]
    DecodedPublicationBudgetExceeded { requested: usize, limit: usize },
    #[error("preview connection needs {requested} bytes; limit is {limit}")]
    ConnectionBudgetExceeded { requested: usize, limit: usize },
    #[error("preview reassembly state needs {requested} bytes; limit is {limit}")]
    ReassemblyStateBudgetExceeded { requested: usize, limit: usize },
    #[error("preview chunk arrived without publication start")]
    MissingPublicationStart,
    #[error("preview chunk duplicates already received data")]
    DuplicateChunk,
    #[error("preview chunks are not contiguous")]
    NonContiguousChunk,
    #[error("preview chunk metadata changed within a publication")]
    MetadataChanged,
    #[error("preview dimensions must be nonzero, got {width}x{height}")]
    InvalidGeometry { width: u32, height: u32 },
    #[error("raw preview publication length must be {expected} bytes, got {actual}")]
    RawPublicationLengthMismatch { expected: usize, actual: usize },
    #[error(
        "preview publication {publication_id} is not newer than stream high-water {high_water}"
    )]
    StalePublication {
        publication_id: u64,
        high_water: u64,
    },
    #[error("preview chunk count {chunks} cannot fit in {total_bytes} non-empty bytes")]
    ImpossibleChunkCount { chunks: u32, total_bytes: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScreenZonesFrameHeader {
    frame_number: u32,
    timestamp_ms: u32,
    source_width: u32,
    source_height: u32,
    grid_cols: u32,
    grid_rows: u32,
    letterbox: [u32; 4],
    payload_offset: usize,
}

impl ScreenZonesFrameHeader {
    fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        let tag = input.first().copied().unwrap_or_default();
        let header_len = match tag {
            SCREEN_ZONES_FRAME_TAG => SCREEN_ZONES_FRAME_HEADER_LEN,
            WIDE_SCREEN_ZONES_FRAME_TAG => WIDE_SCREEN_ZONES_FRAME_HEADER_LEN,
            EXTENDED_SCREEN_ZONES_FRAME_TAG => EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN,
            actual => return Err(PreviewFrameDecodeError::UnknownChannel { actual }),
        };
        if input.len() < header_len {
            return Err(PreviewFrameDecodeError::TooShort {
                expected: header_len,
                actual: input.len(),
            });
        }
        let (source_width, source_height, grid_cols, grid_rows, letterbox) =
            if tag == EXTENDED_SCREEN_ZONES_FRAME_TAG {
                (
                    u32::from_le_bytes(input[9..13].try_into().expect("slice has 4 bytes")),
                    u32::from_le_bytes(input[13..17].try_into().expect("slice has 4 bytes")),
                    u32::from_le_bytes(input[17..21].try_into().expect("slice has 4 bytes")),
                    u32::from_le_bytes(input[21..25].try_into().expect("slice has 4 bytes")),
                    [
                        u32::from_le_bytes(input[25..29].try_into().expect("slice has 4 bytes")),
                        u32::from_le_bytes(input[29..33].try_into().expect("slice has 4 bytes")),
                        u32::from_le_bytes(input[33..37].try_into().expect("slice has 4 bytes")),
                        u32::from_le_bytes(input[37..41].try_into().expect("slice has 4 bytes")),
                    ],
                )
            } else if tag == WIDE_SCREEN_ZONES_FRAME_TAG {
                (
                    u32::from_le_bytes(input[9..13].try_into().expect("slice has 4 bytes")),
                    u32::from_le_bytes(input[13..17].try_into().expect("slice has 4 bytes")),
                    u32::from(input[17]),
                    u32::from(input[18]),
                    [
                        u32::from(input[19]),
                        u32::from(input[20]),
                        u32::from(input[21]),
                        u32::from(input[22]),
                    ],
                )
            } else {
                (
                    u32::from(u16::from_le_bytes(
                        input[9..11].try_into().expect("slice has 2 bytes"),
                    )),
                    u32::from(u16::from_le_bytes(
                        input[11..13].try_into().expect("slice has 2 bytes"),
                    )),
                    u32::from(input[13]),
                    u32::from(input[14]),
                    [
                        u32::from(input[15]),
                        u32::from(input[16]),
                        u32::from(input[17]),
                        u32::from(input[18]),
                    ],
                )
            };

        Ok(Self {
            frame_number: u32::from_le_bytes(input[1..5].try_into().expect("slice has 4 bytes")),
            timestamp_ms: u32::from_le_bytes(input[5..9].try_into().expect("slice has 4 bytes")),
            source_width,
            source_height,
            grid_cols,
            grid_rows,
            letterbox,
            payload_offset: header_len,
        })
    }

    fn end_offset(&self, input_len: usize) -> Result<usize, PreviewFrameDecodeError> {
        let payload_len = usize::try_from(self.grid_cols)
            .ok()
            .and_then(|cols| {
                usize::try_from(self.grid_rows)
                    .ok()
                    .and_then(|rows| cols.checked_mul(rows))
            })
            .and_then(|zones| zones.checked_mul(3))
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        let end = self
            .payload_offset
            .checked_add(payload_len)
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        if input_len != end {
            return Err(PreviewFrameDecodeError::PayloadLengthMismatch {
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

/// Header of an identity-prefixed preview frame. Interactive and display
/// previews share one layout — tag, one-byte identity length, timing,
/// dimensions, format, then the identity — so they share one decoder and
/// differ only in the constants below.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IdentityFrameHeader {
    identity: String,
    frame_number: u32,
    timestamp_ms: u32,
    width: u32,
    height: u32,
    format: PreviewPixelFormat,
    payload_offset: usize,
}

/// The tags and bounds that distinguish one identity-prefixed family.
#[derive(Debug, Clone, Copy)]
struct IdentityFrameLayout {
    tag: u8,
    wide_tag: u8,
    prefix_len: usize,
    wide_prefix_len: usize,
    max_identity_bytes: usize,
}

const INTERACTIVE_PREVIEW_LAYOUT: IdentityFrameLayout = IdentityFrameLayout {
    tag: INTERACTIVE_PREVIEW_FRAME_TAG,
    wide_tag: WIDE_INTERACTIVE_PREVIEW_FRAME_TAG,
    prefix_len: INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN,
    wide_prefix_len: WIDE_INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN,
    max_identity_bytes: INTERACTIVE_PREVIEW_ID_MAX_BYTES,
};

const DISPLAY_PREVIEW_LAYOUT: IdentityFrameLayout = IdentityFrameLayout {
    tag: DISPLAY_PREVIEW_FRAME_TAG,
    wide_tag: WIDE_DISPLAY_PREVIEW_FRAME_TAG,
    prefix_len: DISPLAY_PREVIEW_FRAME_PREFIX_LEN,
    wide_prefix_len: WIDE_DISPLAY_PREVIEW_FRAME_PREFIX_LEN,
    max_identity_bytes: DISPLAY_PREVIEW_ID_MAX_BYTES,
};

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
        if self.format.bytes_per_pixel().is_some() && input_len != end {
            return Err(PreviewFrameDecodeError::PayloadLengthMismatch {
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
        if self.format.bytes_per_pixel().is_some() && input_len != end {
            return Err(PreviewFrameDecodeError::PayloadLengthMismatch {
                expected: payload_len,
                actual: input_len.saturating_sub(self.payload_offset),
            });
        }
        Ok(end)
    }
}

impl IdentityFrameHeader {
    fn decode(input: &[u8], layout: IdentityFrameLayout) -> Result<Self, PreviewFrameDecodeError> {
        let wide = input.first() == Some(&layout.wide_tag);
        let prefix_len = if wide {
            layout.wide_prefix_len
        } else {
            layout.prefix_len
        };
        if input.len() < prefix_len {
            return Err(PreviewFrameDecodeError::TooShort {
                expected: prefix_len,
                actual: input.len(),
            });
        }
        if input[0] != layout.tag && !wide {
            return Err(PreviewFrameDecodeError::UnknownChannel { actual: input[0] });
        }

        let id_len = usize::from(input[1]);
        if id_len == 0 {
            return Err(PreviewFrameDecodeError::EmptyPreviewId);
        }
        if id_len > layout.max_identity_bytes {
            return Err(PreviewFrameDecodeError::PreviewIdTooLong {
                maximum: layout.max_identity_bytes,
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
        let identity = std::str::from_utf8(&input[prefix_len..payload_offset])
            .map_err(|_| PreviewFrameDecodeError::InvalidPreviewIdUtf8)?
            .to_owned();
        validate_preview_stream_id(&identity, layout.max_identity_bytes)?;

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
            identity,
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
        if self.format.bytes_per_pixel().is_some() && input_len != end {
            return Err(PreviewFrameDecodeError::PayloadLengthMismatch {
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
#[derive(Debug, Clone)]
pub struct DisplayPreviewFrameView {
    pub device_id: String,
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,
    pub payload: js_sys::Uint8Array,
}

#[cfg(feature = "ws-client-wasm")]
impl DisplayPreviewFrameView {
    pub fn decode_array_buffer(
        buffer: &js_sys::ArrayBuffer,
    ) -> Result<Self, PreviewFrameDecodeError> {
        let data = js_sys::Uint8Array::new(buffer);
        let header = decode_identity_header_from_array(&data, DISPLAY_PREVIEW_LAYOUT)?;
        let end = header.end_offset(data.length() as usize)?;
        if header.format == PreviewPixelFormat::Jpeg {
            validate_jpeg_array_dimensions(
                &data,
                header.payload_offset,
                end,
                header.width,
                header.height,
            )?;
        }

        Ok(Self {
            device_id: header.identity,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: header.width,
            height: header.height,
            format: header.format,
            payload: data.subarray(header.payload_offset as u32, end as u32),
        })
    }
}

/// Read just enough of a JS-owned buffer to decode an identity-prefixed
/// header: the fixed prefix names the identity length, and the identity
/// itself sits between the prefix and the payload.
#[cfg(feature = "ws-client-wasm")]
fn decode_identity_header_from_array(
    data: &js_sys::Uint8Array,
    layout: IdentityFrameLayout,
) -> Result<IdentityFrameHeader, PreviewFrameDecodeError> {
    let prefix_len = if data.get_index(0) == layout.wide_tag {
        layout.wide_prefix_len
    } else {
        layout.prefix_len
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
    IdentityFrameHeader::decode(&header_bytes, layout)
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
        if header.format == PreviewPixelFormat::Jpeg {
            validate_jpeg_array_dimensions(
                &data,
                header.payload_offset,
                end,
                header.width,
                header.height,
            )?;
        }

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
        if header.format == PreviewPixelFormat::Jpeg {
            validate_jpeg_array_dimensions(
                &data,
                header.payload_offset,
                end,
                header.width,
                header.height,
            )?;
        }

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
        let header = decode_identity_header_from_array(&data, INTERACTIVE_PREVIEW_LAYOUT)?;
        let end = header.end_offset(data.length() as usize)?;
        if header.format == PreviewPixelFormat::Jpeg {
            validate_jpeg_array_dimensions(
                &data,
                header.payload_offset,
                end,
                header.width,
                header.height,
            )?;
        }

        Ok(Self {
            preview_id: header.identity,
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
    #[error("preview frame payload must be {expected} bytes, got {actual}")]
    PayloadLengthMismatch { expected: usize, actual: usize },
    #[error("JPEG preview does not contain readable intrinsic dimensions")]
    JpegDimensionsUnavailable,
    #[error(
        "JPEG intrinsic dimensions {actual_width}x{actual_height} do not match {expected_width}x{expected_height}"
    )]
    JpegDimensionsMismatch {
        expected_width: u32,
        expected_height: u32,
        actual_width: u32,
        actual_height: u32,
    },
    #[error("interactive preview id cannot be empty")]
    EmptyPreviewId,
    #[error("interactive preview id exceeds {maximum} bytes: got {actual}")]
    PreviewIdTooLong { maximum: usize, actual: usize },
    #[error("interactive preview id is not valid UTF-8")]
    InvalidPreviewIdUtf8,
    #[error("interactive preview id contains a control character")]
    InvalidPreviewIdCharacter,
}

/// Validate a client-supplied stream identity: the interactive preview
/// id and the display preview's device id ride the same header slot
/// behind the same one-byte length, so they answer to the same rules.
fn validate_preview_stream_id(id: &str, maximum: usize) -> Result<(), PreviewFrameDecodeError> {
    if id.is_empty() {
        return Err(PreviewFrameDecodeError::EmptyPreviewId);
    }
    if id.len() > maximum {
        return Err(PreviewFrameDecodeError::PreviewIdTooLong {
            maximum,
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
    payload: &[u8],
) -> Result<(), PreviewFrameDecodeError> {
    if let Some(bytes_per_pixel) = format.bytes_per_pixel() {
        let expected = raw_payload_len(width, height, bytes_per_pixel)?;
        if payload.len() != expected {
            return Err(PreviewFrameDecodeError::PayloadLengthMismatch {
                expected,
                actual: payload.len(),
            });
        }
        return Ok(());
    }

    let (actual_width, actual_height) = jpeg_dimensions(payload)?;
    if actual_width != width || actual_height != height {
        return Err(PreviewFrameDecodeError::JpegDimensionsMismatch {
            expected_width: width,
            expected_height: height,
            actual_width,
            actual_height,
        });
    }
    Ok(())
}

fn jpeg_dimensions(payload: &[u8]) -> Result<(u32, u32), PreviewFrameDecodeError> {
    jpeg_dimensions_from_reader(payload.len(), |index| payload.get(index).copied())
}

#[cfg(feature = "ws-client-wasm")]
fn validate_jpeg_array_dimensions(
    data: &js_sys::Uint8Array,
    payload_offset: usize,
    end: usize,
    width: u32,
    height: u32,
) -> Result<(), PreviewFrameDecodeError> {
    let payload_len = end
        .checked_sub(payload_offset)
        .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
    let (actual_width, actual_height) = jpeg_dimensions_from_reader(payload_len, |index| {
        let absolute = payload_offset.checked_add(index)?;
        let absolute = u32::try_from(absolute).ok()?;
        (absolute < data.length()).then(|| data.get_index(absolute))
    })?;
    if actual_width != width || actual_height != height {
        return Err(PreviewFrameDecodeError::JpegDimensionsMismatch {
            expected_width: width,
            expected_height: height,
            actual_width,
            actual_height,
        });
    }
    Ok(())
}

fn jpeg_dimensions_from_reader(
    payload_len: usize,
    mut byte_at: impl FnMut(usize) -> Option<u8>,
) -> Result<(u32, u32), PreviewFrameDecodeError> {
    if byte_at(0) != Some(0xFF) || byte_at(1) != Some(0xD8) {
        return Err(PreviewFrameDecodeError::JpegDimensionsUnavailable);
    }
    let mut offset = 2_usize;
    while offset < payload_len {
        while byte_at(offset) == Some(0xFF) {
            offset = offset.saturating_add(1);
        }
        let marker = byte_at(offset).ok_or(PreviewFrameDecodeError::JpegDimensionsUnavailable)?;
        offset = offset.saturating_add(1);
        if marker == 0x00 || marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        if marker == 0xDA {
            return Err(PreviewFrameDecodeError::JpegDimensionsUnavailable);
        }
        let segment_len = usize::from(u16::from_be_bytes([
            byte_at(offset).ok_or(PreviewFrameDecodeError::JpegDimensionsUnavailable)?,
            byte_at(offset.saturating_add(1))
                .ok_or(PreviewFrameDecodeError::JpegDimensionsUnavailable)?,
        ]));
        if segment_len < 2 {
            return Err(PreviewFrameDecodeError::JpegDimensionsUnavailable);
        }
        let segment_end = offset
            .checked_add(segment_len)
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        if segment_end > payload_len {
            return Err(PreviewFrameDecodeError::JpegDimensionsUnavailable);
        }
        if matches!(
            marker,
            0xC0 | 0xC1
                | 0xC2
                | 0xC3
                | 0xC5
                | 0xC6
                | 0xC7
                | 0xC9
                | 0xCA
                | 0xCB
                | 0xCD
                | 0xCE
                | 0xCF
        ) {
            if segment_len < 7 {
                return Err(PreviewFrameDecodeError::JpegDimensionsUnavailable);
            }
            let height = u32::from(u16::from_be_bytes([
                byte_at(offset.saturating_add(3))
                    .ok_or(PreviewFrameDecodeError::JpegDimensionsUnavailable)?,
                byte_at(offset.saturating_add(4))
                    .ok_or(PreviewFrameDecodeError::JpegDimensionsUnavailable)?,
            ]));
            let width = u32::from(u16::from_be_bytes([
                byte_at(offset.saturating_add(5))
                    .ok_or(PreviewFrameDecodeError::JpegDimensionsUnavailable)?,
                byte_at(offset.saturating_add(6))
                    .ok_or(PreviewFrameDecodeError::JpegDimensionsUnavailable)?,
            ]));
            if width == 0 || height == 0 {
                return Err(PreviewFrameDecodeError::JpegDimensionsUnavailable);
            }
            return Ok((width, height));
        }
        offset = segment_end;
    }
    Err(PreviewFrameDecodeError::JpegDimensionsUnavailable)
}
