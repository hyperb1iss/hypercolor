//! Push 2 display frame encoding.
//!
//! The Push 2 display is a 960x160 RGB565 panel accessed over USB bulk transfer.
//! Each frame is XOR-masked with a repeating 4-byte pattern and sent in 16 KiB
//! chunks preceded by a magic header.

use std::time::Duration;

use image::{ImageFormat, imageops::FilterType};
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

pub(super) fn encode_display_frame_from_jpeg(
    jpeg_data: &[u8],
    commands: &mut Vec<ProtocolCommand>,
) -> Option<()> {
    let image = image::load_from_memory_with_format(jpeg_data, ImageFormat::Jpeg).ok()?;
    let rgb = image
        .resize_exact(
            u32::try_from(PUSH2_DISPLAY_WIDTH).unwrap_or(960),
            u32::try_from(PUSH2_DISPLAY_HEIGHT).unwrap_or(160),
            FilterType::Nearest,
        )
        .to_rgb8();
    build_display_commands(rgb.as_raw(), commands);
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
    for (index, byte) in line.pixels.iter_mut().enumerate() {
        *byte ^= PUSH2_DISPLAY_XOR_MASK[index & 3];
    }
    for (index, byte) in line.padding.iter_mut().enumerate() {
        *byte ^= PUSH2_DISPLAY_XOR_MASK[(PUSH2_DISPLAY_LINE_PIXELS + index) & 3];
    }
}
