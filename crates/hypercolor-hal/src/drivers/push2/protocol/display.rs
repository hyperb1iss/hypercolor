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
const PUSH2_DISPLAY_LINES_PER_TRANSFER: usize =
    PUSH2_DISPLAY_TRANSFER_CHUNK / PUSH2_DISPLAY_LINE_SIZE;

const _: () = assert!(
    std::mem::size_of::<Push2DisplayHeader>() == 16,
    "Push2DisplayHeader must be exactly 16 bytes"
);
const _: () = assert!(
    PUSH2_DISPLAY_LINE_SIZE == PUSH2_DISPLAY_LINE_PIXELS + PUSH2_DISPLAY_LINE_PADDING,
    "Push2 display line must be exactly 2048 bytes"
);

#[derive(Default)]
pub(super) struct Push2DisplayEncoder {
    cached_jpeg: Vec<u8>,
    rgb_buffer: Vec<u8>,
    turbojpeg: Option<TurboJpegDecompressor>,
}

impl Push2DisplayEncoder {
    pub(super) fn encode_display_frame_from_jpeg(
        &mut self,
        jpeg_data: &[u8],
        commands: &mut Vec<ProtocolCommand>,
    ) -> Option<()> {
        if !commands.is_empty() && self.cached_jpeg == jpeg_data {
            return Some(());
        }

        encode_display_frame_uncached(
            jpeg_data,
            commands,
            &mut self.rgb_buffer,
            &mut self.turbojpeg,
        )?;

        self.cached_jpeg.clear();
        self.cached_jpeg.extend_from_slice(jpeg_data);
        Some(())
    }
}

fn encode_display_frame_uncached(
    jpeg_data: &[u8],
    commands: &mut Vec<ProtocolCommand>,
    rgb_buffer: &mut Vec<u8>,
    turbojpeg: &mut Option<TurboJpegDecompressor>,
) -> Option<()> {
    if decode_jpeg_into_rgb_buffer(jpeg_data, rgb_buffer, turbojpeg).is_some() {
        build_display_commands(rgb_buffer.as_slice(), commands);
        return Some(());
    }

    let image = image::load_from_memory_with_format(jpeg_data, ImageFormat::Jpeg).ok()?;
    let rgb = if image.width() == 960 && image.height() == 160 {
        image.into_rgb8()
    } else {
        image
            .resize_exact(960, 160, FilterType::Nearest)
            .into_rgb8()
    };
    build_display_commands(rgb.as_raw(), commands);
    Some(())
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

fn build_display_commands(rgb_bytes: &[u8], commands: &mut Vec<ProtocolCommand>) {
    let mut buffer = CommandBuffer::new(commands);
    buffer.push_struct(
        &DISPLAY_HEADER,
        false,
        Duration::ZERO,
        Duration::ZERO,
        TransferType::Bulk,
    );

    for first_row in (0..PUSH2_DISPLAY_HEIGHT).step_by(PUSH2_DISPLAY_LINES_PER_TRANSFER) {
        let row_count = PUSH2_DISPLAY_LINES_PER_TRANSFER.min(PUSH2_DISPLAY_HEIGHT - first_row);
        buffer.push_fill(
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Bulk,
            |chunk| {
                chunk.resize(row_count * PUSH2_DISPLAY_LINE_SIZE, 0);
                for local_row in 0..row_count {
                    encode_display_line_into(
                        rgb_bytes,
                        first_row + local_row,
                        &mut chunk[local_row * PUSH2_DISPLAY_LINE_SIZE
                            ..(local_row + 1) * PUSH2_DISPLAY_LINE_SIZE],
                    );
                }
            },
        );
    }

    buffer.finish();
}

fn encode_rgb565(red: u8, green: u8, blue: u8) -> [u8; 2] {
    let encoded = (u16::from(blue >> 3) << 11) | (u16::from(green >> 2) << 5) | u16::from(red >> 3);
    encoded.to_le_bytes()
}

fn encode_display_line_into(rgb_bytes: &[u8], row: usize, line: &mut [u8]) {
    let row_start = row * PUSH2_DISPLAY_WIDTH * 3;

    for column in 0..PUSH2_DISPLAY_WIDTH {
        let rgb_offset = row_start + column * 3;
        let pixel_offset = column * 2;
        let encoded = encode_rgb565(
            rgb_bytes[rgb_offset],
            rgb_bytes[rgb_offset + 1],
            rgb_bytes[rgb_offset + 2],
        );
        line[pixel_offset] = encoded[0] ^ PUSH2_DISPLAY_XOR_MASK[pixel_offset & 3];
        line[pixel_offset + 1] = encoded[1] ^ PUSH2_DISPLAY_XOR_MASK[(pixel_offset + 1) & 3];
    }

    for index in 0..PUSH2_DISPLAY_LINE_PADDING {
        line[PUSH2_DISPLAY_LINE_PIXELS + index] = PUSH2_DISPLAY_XOR_MASK[index & 3];
    }
}
