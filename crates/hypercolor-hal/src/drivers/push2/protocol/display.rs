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
use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

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

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct Push2DisplayLine {
    pixels: [u8; PUSH2_DISPLAY_LINE_PIXELS],
    padding: [u8; PUSH2_DISPLAY_LINE_PADDING],
}

const DISPLAY_HEADER: Push2DisplayHeader = Push2DisplayHeader {
    magic: [0xFF, 0xCC, 0xAA, 0x88],
    padding: [0; 12],
};

const _: () = assert!(
    std::mem::size_of::<Push2DisplayHeader>() == 16,
    "Push2DisplayHeader must be exactly 16 bytes"
);
const _: () = assert!(
    std::mem::size_of::<Push2DisplayLine>() == PUSH2_DISPLAY_LINE_SIZE,
    "Push2DisplayLine must be exactly 2048 bytes"
);

#[derive(Default)]
pub(super) struct Push2DisplayEncoder {
    cached_jpeg: Vec<u8>,
    cached_commands: Vec<ProtocolCommand>,
    rgb_buffer: Vec<u8>,
    turbojpeg: Option<TurboJpegDecompressor>,
}

impl Push2DisplayEncoder {
    pub(super) fn encode_display_frame_from_jpeg(
        &mut self,
        jpeg_data: &[u8],
        commands: &mut Vec<ProtocolCommand>,
    ) -> Option<()> {
        if !self.cached_commands.is_empty() && self.cached_jpeg == jpeg_data {
            commands.clone_from(&self.cached_commands);
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
        self.cached_commands.clone_from(commands);
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
    let mut transfer_chunk = Vec::with_capacity(PUSH2_DISPLAY_TRANSFER_CHUNK);
    buffer.push_struct(
        &DISPLAY_HEADER,
        false,
        Duration::ZERO,
        Duration::ZERO,
        TransferType::Bulk,
    );

    for row in 0..PUSH2_DISPLAY_HEIGHT {
        let row_start = row * PUSH2_DISPLAY_WIDTH * 3;
        let mut line = Push2DisplayLine::new_zeroed();

        for column in 0..PUSH2_DISPLAY_WIDTH {
            let rgb_offset = row_start + column * 3;
            let pixel_offset = column * 2;
            let encoded = encode_rgb565(
                rgb_bytes[rgb_offset],
                rgb_bytes[rgb_offset + 1],
                rgb_bytes[rgb_offset + 2],
            );
            line.pixels[pixel_offset..pixel_offset + 2].copy_from_slice(&encoded);
        }

        xor_shape_line(&mut line);

        if transfer_chunk.len() + PUSH2_DISPLAY_LINE_SIZE > PUSH2_DISPLAY_TRANSFER_CHUNK
            && !transfer_chunk.is_empty()
        {
            buffer.push_slice(
                &transfer_chunk,
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Bulk,
            );
            transfer_chunk.clear();
        }

        transfer_chunk.extend_from_slice(line.as_bytes());
    }

    if !transfer_chunk.is_empty() {
        buffer.push_slice(
            &transfer_chunk,
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Bulk,
        );
    }

    buffer.finish();
}

fn encode_rgb565(red: u8, green: u8, blue: u8) -> [u8; 2] {
    let encoded = (u16::from(blue >> 3) << 11) | (u16::from(green >> 2) << 5) | u16::from(red >> 3);
    encoded.to_le_bytes()
}

fn xor_shape_line(line: &mut Push2DisplayLine) {
    for chunk in line.pixels.chunks_exact_mut(4) {
        chunk[0] ^= PUSH2_DISPLAY_XOR_MASK[0];
        chunk[1] ^= PUSH2_DISPLAY_XOR_MASK[1];
        chunk[2] ^= PUSH2_DISPLAY_XOR_MASK[2];
        chunk[3] ^= PUSH2_DISPLAY_XOR_MASK[3];
    }
    for chunk in line.padding.chunks_exact_mut(4) {
        chunk[0] ^= PUSH2_DISPLAY_XOR_MASK[0];
        chunk[1] ^= PUSH2_DISPLAY_XOR_MASK[1];
        chunk[2] ^= PUSH2_DISPLAY_XOR_MASK[2];
        chunk[3] ^= PUSH2_DISPLAY_XOR_MASK[3];
    }
}
