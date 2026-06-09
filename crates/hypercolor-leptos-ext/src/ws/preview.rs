use bytes::{BufMut, Bytes, BytesMut};
use thiserror::Error;

pub const PREVIEW_FRAME_HEADER_LEN: usize = 14;
pub const ZONE_PREVIEW_FRAME_HEADER_LEN: usize = 46;
pub const ZONE_PREVIEW_FRAME_TAG: u8 = 0x08;

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
    pub width: u16,
    pub height: u16,
    pub format: PreviewPixelFormat,
    pub payload: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZonePreviewFrame {
    pub scene_id: [u8; 16],
    pub zone_id: [u8; 16],
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u16,
    pub height: u16,
    pub format: PreviewPixelFormat,
    pub payload: Bytes,
}

impl PreviewFrame {
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        PREVIEW_FRAME_HEADER_LEN.saturating_add(self.payload.len())
    }

    #[must_use]
    pub fn encode(&self) -> Bytes {
        let mut out = BytesMut::with_capacity(self.encoded_len());
        out.put_u8(self.channel.tag());
        out.put_u32_le(self.frame_number);
        out.put_u32_le(self.timestamp_ms);
        out.put_u16_le(self.width);
        out.put_u16_le(self.height);
        out.put_u8(self.format.tag());
        out.extend_from_slice(&self.payload);
        out.freeze()
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
            payload: Bytes::copy_from_slice(&input[PREVIEW_FRAME_HEADER_LEN..end]),
        })
    }
}

impl ZonePreviewFrame {
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        ZONE_PREVIEW_FRAME_HEADER_LEN.saturating_add(self.payload.len())
    }

    #[must_use]
    pub fn encode(&self) -> Bytes {
        let mut out = BytesMut::with_capacity(self.encoded_len());
        out.put_u8(ZONE_PREVIEW_FRAME_TAG);
        out.put_u32_le(self.frame_number);
        out.put_u32_le(self.timestamp_ms);
        out.extend_from_slice(&self.scene_id);
        out.extend_from_slice(&self.zone_id);
        out.put_u16_le(self.width);
        out.put_u16_le(self.height);
        out.put_u8(self.format.tag());
        out.extend_from_slice(&self.payload);
        out.freeze()
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
            payload: Bytes::copy_from_slice(&input[ZONE_PREVIEW_FRAME_HEADER_LEN..end]),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PreviewFrameHeader {
    channel: PreviewFrameChannel,
    frame_number: u32,
    timestamp_ms: u32,
    width: u16,
    height: u16,
    format: PreviewPixelFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ZonePreviewFrameHeader {
    scene_id: [u8; 16],
    zone_id: [u8; 16],
    frame_number: u32,
    timestamp_ms: u32,
    width: u16,
    height: u16,
    format: PreviewPixelFormat,
}

impl PreviewFrameHeader {
    fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        if input.len() < PREVIEW_FRAME_HEADER_LEN {
            return Err(PreviewFrameDecodeError::TooShort {
                expected: PREVIEW_FRAME_HEADER_LEN,
                actual: input.len(),
            });
        }

        Ok(Self {
            channel: PreviewFrameChannel::try_from(input[0])?,
            frame_number: u32::from_le_bytes(input[1..5].try_into().expect("slice has 4 bytes")),
            timestamp_ms: u32::from_le_bytes(input[5..9].try_into().expect("slice has 4 bytes")),
            width: u16::from_le_bytes(input[9..11].try_into().expect("slice has 2 bytes")),
            height: u16::from_le_bytes(input[11..13].try_into().expect("slice has 2 bytes")),
            format: PreviewPixelFormat::try_from(input[13])?,
        })
    }

    fn end_offset(&self, input_len: usize) -> Result<usize, PreviewFrameDecodeError> {
        let payload_len = match self.format.bytes_per_pixel() {
            Some(bytes_per_pixel) => raw_payload_len(self.width, self.height, bytes_per_pixel)?,
            None => input_len - PREVIEW_FRAME_HEADER_LEN,
        };
        let end = PREVIEW_FRAME_HEADER_LEN
            .checked_add(payload_len)
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        if input_len < end {
            return Err(PreviewFrameDecodeError::PayloadTooShort {
                expected: payload_len,
                actual: input_len.saturating_sub(PREVIEW_FRAME_HEADER_LEN),
            });
        }
        Ok(end)
    }
}

impl ZonePreviewFrameHeader {
    fn decode(input: &[u8]) -> Result<Self, PreviewFrameDecodeError> {
        if input.len() < ZONE_PREVIEW_FRAME_HEADER_LEN {
            return Err(PreviewFrameDecodeError::TooShort {
                expected: ZONE_PREVIEW_FRAME_HEADER_LEN,
                actual: input.len(),
            });
        }
        if input[0] != ZONE_PREVIEW_FRAME_TAG {
            return Err(PreviewFrameDecodeError::UnknownChannel { actual: input[0] });
        }

        Ok(Self {
            frame_number: u32::from_le_bytes(input[1..5].try_into().expect("slice has 4 bytes")),
            timestamp_ms: u32::from_le_bytes(input[5..9].try_into().expect("slice has 4 bytes")),
            scene_id: input[9..25].try_into().expect("slice has 16 bytes"),
            zone_id: input[25..41].try_into().expect("slice has 16 bytes"),
            width: u16::from_le_bytes(input[41..43].try_into().expect("slice has 2 bytes")),
            height: u16::from_le_bytes(input[43..45].try_into().expect("slice has 2 bytes")),
            format: PreviewPixelFormat::try_from(input[45])?,
        })
    }

    fn end_offset(&self, input_len: usize) -> Result<usize, PreviewFrameDecodeError> {
        let payload_len = match self.format.bytes_per_pixel() {
            Some(bytes_per_pixel) => raw_payload_len(self.width, self.height, bytes_per_pixel)?,
            None => input_len - ZONE_PREVIEW_FRAME_HEADER_LEN,
        };
        let end = ZONE_PREVIEW_FRAME_HEADER_LEN
            .checked_add(payload_len)
            .ok_or(PreviewFrameDecodeError::DimensionsOverflow)?;
        if input_len < end {
            return Err(PreviewFrameDecodeError::PayloadTooShort {
                expected: payload_len,
                actual: input_len.saturating_sub(ZONE_PREVIEW_FRAME_HEADER_LEN),
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
impl PreviewFrameView {
    pub fn decode_array_buffer(
        buffer: &js_sys::ArrayBuffer,
    ) -> Result<Self, PreviewFrameDecodeError> {
        let data = js_sys::Uint8Array::new(buffer);
        let header_bytes = data.subarray(0, PREVIEW_FRAME_HEADER_LEN as u32).to_vec();
        let header = PreviewFrameHeader::decode(&header_bytes)?;
        let end = header.end_offset(data.length() as usize)?;

        Ok(Self {
            channel: header.channel,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: u32::from(header.width),
            height: u32::from(header.height),
            format: header.format,
            payload: data.subarray(PREVIEW_FRAME_HEADER_LEN as u32, end as u32),
        })
    }

    #[must_use]
    pub fn pixel_count(&self) -> usize {
        let width = usize::try_from(self.width).unwrap_or(0);
        let height = usize::try_from(self.height).unwrap_or(0);
        width.saturating_mul(height)
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
                let mut rgba = vec![0_u8; (len / 3).saturating_mul(4)];
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
        let header_bytes = data
            .subarray(0, ZONE_PREVIEW_FRAME_HEADER_LEN as u32)
            .to_vec();
        let header = ZonePreviewFrameHeader::decode(&header_bytes)?;
        let end = header.end_offset(data.length() as usize)?;

        Ok(Self {
            scene_id: header.scene_id,
            zone_id: header.zone_id,
            frame_number: header.frame_number,
            timestamp_ms: header.timestamp_ms,
            width: u32::from(header.width),
            height: u32::from(header.height),
            format: header.format,
            payload: data.subarray(ZONE_PREVIEW_FRAME_HEADER_LEN as u32, end as u32),
        })
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
    #[error("preview frame payload is too short: expected {expected} bytes, got {actual}")]
    PayloadTooShort { expected: usize, actual: usize },
}

fn raw_payload_len(
    width: u16,
    height: u16,
    bytes_per_pixel: usize,
) -> Result<usize, PreviewFrameDecodeError> {
    usize::from(width)
        .checked_mul(usize::from(height))
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .ok_or(PreviewFrameDecodeError::DimensionsOverflow)
}
