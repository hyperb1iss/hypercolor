//! Push 2 display frame encoding.
//!
//! The Push 2 display is a 960x160 RGB565 panel accessed over USB bulk transfer.
//! Each frame is XOR-masked with a repeating 4-byte pattern and sent in 16 KiB
//! chunks preceded by a magic header.

use std::time::Duration;

use image::{ImageFormat, imageops::FilterType};
use turbojpeg::{
    Decompressor as TurboJpegDecompressor, Image as TurboJpegImage,
    PixelFormat as TurboJpegPixelFormat,
};
use zerocopy::{Immutable, IntoBytes, KnownLayout};

use crate::display::{
    ChunkCommandPolicy, ChunkContext, DisplayChunkLayout, DisplayEncodeError, LineRepack,
    Packed16Format, encode_chunked_display_frame_into,
};
use crate::protocol::{CommandBuffer, ProtocolCommand, TransferType};

use super::{
    PUSH2_DISPLAY_HEIGHT, PUSH2_DISPLAY_LINE_PADDING, PUSH2_DISPLAY_LINE_PIXELS,
    PUSH2_DISPLAY_LINE_SIZE, PUSH2_DISPLAY_TRANSFER_CHUNK, PUSH2_DISPLAY_WIDTH,
    PUSH2_DISPLAY_XOR_MASK,
};

#[derive(IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct Push2DisplayHeader {
    magic: [u8; 4],
    padding: [u8; 12],
}

const DISPLAY_HEADER: Push2DisplayHeader = Push2DisplayHeader {
    magic: [0xFF, 0xCC, 0xAA, 0x88],
    padding: [0; 12],
};
const PUSH2_DISPLAY_FRAME_BYTES: usize = PUSH2_DISPLAY_LINE_SIZE * PUSH2_DISPLAY_HEIGHT;

/// Pixel stage: RGB888 to the panel's masked BGR565 lines.
const PUSH2_DISPLAY_REPACK: LineRepack<'static> = LineRepack {
    width: PUSH2_DISPLAY_WIDTH,
    height: PUSH2_DISPLAY_HEIGHT,
    format: Packed16Format::Bgr565,
    line_len: PUSH2_DISPLAY_LINE_SIZE,
    filler: 0x00,
    xor_mask: &PUSH2_DISPLAY_XOR_MASK,
};

const _: () = assert!(
    std::mem::size_of::<Push2DisplayHeader>() == 16,
    "Push2DisplayHeader must be exactly 16 bytes"
);
const _: () = assert!(
    PUSH2_DISPLAY_LINE_SIZE == PUSH2_DISPLAY_LINE_PIXELS + PUSH2_DISPLAY_LINE_PADDING,
    "Push2 display line must be exactly 2048 bytes"
);
const _: () = assert!(
    PUSH2_DISPLAY_FRAME_BYTES.is_multiple_of(PUSH2_DISPLAY_TRANSFER_CHUNK),
    "Push2 frames must divide into whole transfer chunks; a short final chunk \
     would be zero-padded up to the chunk size and desync the panel"
);

/// Framing stage: fixed 16 KiB bulk chunks with no per-chunk header.
///
/// The frame's magic header is one command ahead of the chunk stream, not a
/// per-packet prefix, so the layout writes no header of its own.
struct Push2DisplayLayout;

impl DisplayChunkLayout for Push2DisplayLayout {
    fn packet_len(&self) -> usize {
        PUSH2_DISPLAY_TRANSFER_CHUNK
    }

    fn max_payload(&self) -> usize {
        PUSH2_DISPLAY_TRANSFER_CHUNK
    }

    fn payload_offset(&self) -> usize {
        0
    }

    fn write_header(&self, _packet: &mut [u8], _ctx: &ChunkContext<'_>) {}

    fn command_policy(&self, _ctx: &ChunkContext<'_>) -> ChunkCommandPolicy {
        ChunkCommandPolicy::fire_and_forget(TransferType::Bulk)
    }

    fn max_chunks(&self) -> u32 {
        // The panel counts chunks itself; nothing on the wire numbers them.
        u32::MAX
    }
}

#[derive(Default)]
pub(super) struct Push2DisplayEncoder {
    cached_jpeg: Vec<u8>,
    rgb_buffer: Vec<u8>,
    frame_buffer: Vec<u8>,
    turbojpeg: Option<TurboJpegDecompressor>,
}

impl Push2DisplayEncoder {
    pub(super) fn encode_display_frame_from_jpeg(
        &mut self,
        jpeg_data: &[u8],
        commands: &mut Vec<ProtocolCommand>,
    ) -> Result<(), DisplayEncodeError> {
        if !commands.is_empty() && self.cached_jpeg == jpeg_data {
            return Ok(());
        }

        encode_display_frame_uncached(
            jpeg_data,
            commands,
            &mut self.rgb_buffer,
            &mut self.frame_buffer,
            &mut self.turbojpeg,
        )?;

        self.cached_jpeg.clear();
        self.cached_jpeg.extend_from_slice(jpeg_data);
        Ok(())
    }

    pub(super) fn encode_display_frame_from_rgb(
        &mut self,
        width: u32,
        height: u32,
        rgb_data: &[u8],
        commands: &mut Vec<ProtocolCommand>,
    ) -> Result<(), DisplayEncodeError> {
        let expected_width = u32::try_from(PUSH2_DISPLAY_WIDTH).unwrap_or(u32::MAX);
        let expected_height = u32::try_from(PUSH2_DISPLAY_HEIGHT).unwrap_or(u32::MAX);
        if width != expected_width || height != expected_height {
            return Err(DisplayEncodeError::WrongGeometry {
                expected_width,
                expected_height,
                width,
                height,
            });
        }
        let expected_len = PUSH2_DISPLAY_WIDTH * PUSH2_DISPLAY_HEIGHT * 3;
        if rgb_data.len() != expected_len {
            return Err(DisplayEncodeError::Undecodable {
                detail: format!(
                    "RGB888 payload is {} bytes, {expected_len} needed",
                    rgb_data.len()
                ),
            });
        }

        self.cached_jpeg.clear();
        build_display_commands(rgb_data, &mut self.frame_buffer, commands)
    }
}

fn encode_display_frame_uncached(
    jpeg_data: &[u8],
    commands: &mut Vec<ProtocolCommand>,
    rgb_buffer: &mut Vec<u8>,
    frame_buffer: &mut Vec<u8>,
    turbojpeg: &mut Option<TurboJpegDecompressor>,
) -> Result<(), DisplayEncodeError> {
    if decode_jpeg_into_rgb_buffer(jpeg_data, rgb_buffer, turbojpeg).is_some() {
        return build_display_commands(rgb_buffer.as_slice(), frame_buffer, commands);
    }

    let image =
        image::load_from_memory_with_format(jpeg_data, ImageFormat::Jpeg).map_err(|error| {
            DisplayEncodeError::Undecodable {
                detail: error.to_string(),
            }
        })?;
    let rgb = if image.width() == 960 && image.height() == 160 {
        image.into_rgb8()
    } else {
        image
            .resize_exact(960, 160, FilterType::Nearest)
            .into_rgb8()
    };
    build_display_commands(rgb.as_raw(), frame_buffer, commands)
}

fn decode_jpeg_into_rgb_buffer(
    jpeg_data: &[u8],
    rgb_buffer: &mut Vec<u8>,
    turbojpeg: &mut Option<TurboJpegDecompressor>,
) -> Option<()> {
    if turbojpeg.is_none() {
        *turbojpeg = TurboJpegDecompressor::new().ok();
    }

    let decompressor = turbojpeg.as_mut()?;
    let header = decompressor.read_header(jpeg_data).ok()?;
    if header.width != PUSH2_DISPLAY_WIDTH || header.height != PUSH2_DISPLAY_HEIGHT {
        return None;
    }

    let pixel_format = TurboJpegPixelFormat::RGB;
    let pitch = PUSH2_DISPLAY_WIDTH.checked_mul(pixel_format.size())?;
    let required_len = pitch.checked_mul(PUSH2_DISPLAY_HEIGHT)?;
    if rgb_buffer.len() != required_len {
        rgb_buffer.resize(required_len, 0);
    }

    decompressor
        .decompress(
            jpeg_data,
            TurboJpegImage {
                pixels: rgb_buffer.as_mut_slice(),
                width: PUSH2_DISPLAY_WIDTH,
                pitch,
                height: PUSH2_DISPLAY_HEIGHT,
                format: pixel_format,
            },
        )
        .ok()?;
    Some(())
}

/// Repack one decoded frame and emit the magic header plus its bulk chunks.
///
/// `frame_buffer` is the encoder's reusable packed-pixel scratch; the repack
/// rewrites every byte of it, so nothing survives from the previous frame.
fn build_display_commands(
    rgb_bytes: &[u8],
    frame_buffer: &mut Vec<u8>,
    commands: &mut Vec<ProtocolCommand>,
) -> Result<(), DisplayEncodeError> {
    PUSH2_DISPLAY_REPACK.repack_rgb888(rgb_bytes, frame_buffer)?;

    let mut buffer = CommandBuffer::new(commands);
    buffer.push_struct(
        &DISPLAY_HEADER,
        false,
        Duration::ZERO,
        Duration::ZERO,
        TransferType::Bulk,
    );
    let framed = encode_chunked_display_frame_into(&Push2DisplayLayout, frame_buffer, &mut buffer);
    buffer.finish();

    // The preamble command is already in the buffer; on failure the caller
    // must not send it alone, so the buffer is emptied with the error.
    if framed.is_err() {
        commands.clear();
    }
    framed
}
