use anyhow::{Context, Result};
use turbojpeg::{
    Compressor as TurboJpegCompressor, Image as TurboJpegImage,
    PixelFormat as TurboJpegPixelFormat, Subsamp as TurboJpegSubsamp,
    compressed_buf_len as turbojpeg_compressed_buf_len,
};

use hypercolor_core::bus::CanvasFrame;
use hypercolor_types::canvas::{linear_to_srgb_u8, srgb_u8_to_linear};

const CANVAS_HEADER_LEN: usize = 14;
const PREVIEW_JPEG_QUALITY: u8 = 80;
const PREVIEW_JPEG_SUBSAMP: TurboJpegSubsamp = TurboJpegSubsamp::Sub2x2;
const JPEG_FORMAT_TAG: u8 = 2;

pub(super) struct PreviewJpegEncoder {
    rgba_buffer: Vec<u8>,
    jpeg_buffer: Vec<u8>,
    jpeg_compressor: TurboJpegCompressor,
    brightness_bits: u32,
    brightness_lut: [u8; 256],
}

impl PreviewJpegEncoder {
    pub(super) fn new() -> Result<Self> {
        let mut jpeg_compressor =
            TurboJpegCompressor::new().context("failed to initialize preview JPEG encoder")?;
        jpeg_compressor
            .set_quality(i32::from(PREVIEW_JPEG_QUALITY))
            .context("failed to configure preview JPEG quality")?;
        jpeg_compressor
            .set_subsamp(PREVIEW_JPEG_SUBSAMP)
            .context("failed to configure preview JPEG subsampling")?;

        Ok(Self {
            rgba_buffer: Vec::new(),
            jpeg_buffer: Vec::new(),
            jpeg_compressor,
            brightness_bits: 1.0_f32.to_bits(),
            brightness_lut: identity_brightness_lut(),
        })
    }

    pub(super) fn encode_scaled_payload(
        &mut self,
        frame: &CanvasFrame,
        header: u8,
        brightness: f32,
        requested_width: u32,
        requested_height: u32,
    ) -> Result<Vec<u8>> {
        let (target_width, target_height) =
            resolve_preview_dimensions(frame.width, frame.height, requested_width, requested_height);
        let mut jpeg = self.encode_scaled_body(
            frame,
            brightness,
            requested_width,
            requested_height,
        )?;
        let width_u16 = u16::try_from(target_width).unwrap_or(u16::MAX);
        let height_u16 = u16::try_from(target_height).unwrap_or(u16::MAX);
        let body_offset = CANVAS_HEADER_LEN;
        let payload_len = body_offset.saturating_add(jpeg.len());
        let mut payload = vec![0; payload_len];
        write_canvas_header(
            &mut payload[..body_offset],
            header,
            frame,
            width_u16,
            height_u16,
            JPEG_FORMAT_TAG,
        );
        payload[body_offset..].copy_from_slice(&jpeg);
        jpeg.clear();
        self.jpeg_buffer = jpeg;
        Ok(payload)
    }

    #[cfg(test)]
    pub(super) fn encode(
        &mut self,
        frame: &CanvasFrame,
        header: u8,
        brightness: f32,
    ) -> Result<axum::body::Bytes> {
        self.encode_scaled_payload(frame, header, brightness, 0, 0)
            .map(axum::body::Bytes::from)
    }

    pub(super) fn encode_scaled_body(
        &mut self,
        frame: &CanvasFrame,
        brightness: f32,
        requested_width: u32,
        requested_height: u32,
    ) -> Result<Vec<u8>> {
        let brightness = brightness.clamp(0.0, 1.0);
        let (target_width, target_height) =
            resolve_preview_dimensions(frame.width, frame.height, requested_width, requested_height);
        let width_u16 = u16::try_from(target_width).unwrap_or(u16::MAX);
        let height_u16 = u16::try_from(target_height).unwrap_or(u16::MAX);
        let width = usize::from(width_u16);
        let height = usize::from(height_u16);
        let required_len = turbojpeg_compressed_buf_len(width, height, PREVIEW_JPEG_SUBSAMP)
            .context("failed to size preview JPEG buffer")?;
        let mut jpeg_buffer = std::mem::take(&mut self.jpeg_buffer);
        if jpeg_buffer.len() < required_len {
            jpeg_buffer.resize(required_len, 0);
        } else {
            jpeg_buffer.truncate(required_len);
        }

        let (pixels, pixel_format) = if brightness >= 0.999
            && target_width == frame.width
            && target_height == frame.height
        {
            (frame.rgba_bytes(), TurboJpegPixelFormat::RGBA)
        } else {
            self.refresh_brightness_lut(brightness);
            self.copy_scaled_rgba_pixels(frame, target_width, target_height, brightness);
            (self.rgba_buffer.as_slice(), TurboJpegPixelFormat::RGBA)
        };
        let pitch = width
            .checked_mul(pixel_format.size())
            .context("preview JPEG row pitch overflow")?;
        let image = TurboJpegImage {
            pixels,
            width,
            pitch,
            height,
            format: pixel_format,
        };
        let jpeg_len = self
            .jpeg_compressor
            .compress_to_slice(image, jpeg_buffer.as_mut_slice())
            .context("failed to encode preview JPEG frame")?;
        jpeg_buffer.truncate(jpeg_len);
        Ok(jpeg_buffer)
    }

    fn copy_scaled_rgba_pixels(
        &mut self,
        frame: &CanvasFrame,
        target_width: u32,
        target_height: u32,
        brightness: f32,
    ) {
        let width = usize::try_from(target_width).unwrap_or(0);
        let height = usize::try_from(target_height).unwrap_or(0);
        let pixel_count = width.saturating_mul(height);
        let required_len = pixel_count.saturating_mul(TurboJpegPixelFormat::RGBA.size());
        if self.rgba_buffer.len() != required_len {
            self.rgba_buffer.resize(required_len, 0);
        }
        let rgba = frame.rgba_bytes();
        let source_width = usize::try_from(frame.width).unwrap_or(0);
        let source_height = usize::try_from(frame.height).unwrap_or(0);
        let source_brightness_full = brightness >= 0.999;
        for y in 0..height {
            let source_y = y
                .saturating_mul(source_height)
                .checked_div(height.max(1))
                .unwrap_or(0);
            for x in 0..width {
                let source_x = x
                    .saturating_mul(source_width)
                    .checked_div(width.max(1))
                    .unwrap_or(0);
                let source_offset = source_y
                    .saturating_mul(source_width)
                    .saturating_add(source_x)
                    .saturating_mul(4);
                let out_offset = y.saturating_mul(width).saturating_add(x).saturating_mul(4);
                let pixel = &rgba[source_offset..source_offset + 4];
                if source_brightness_full {
                    self.rgba_buffer[out_offset..out_offset + 4].copy_from_slice(pixel);
                } else {
                    self.rgba_buffer[out_offset] = self.brightness_lut[usize::from(pixel[0])];
                    self.rgba_buffer[out_offset + 1] =
                        self.brightness_lut[usize::from(pixel[1])];
                    self.rgba_buffer[out_offset + 2] =
                        self.brightness_lut[usize::from(pixel[2])];
                    self.rgba_buffer[out_offset + 3] = pixel[3];
                }
            }
        }
    }

    fn refresh_brightness_lut(&mut self, brightness: f32) {
        let brightness_bits = brightness.to_bits();
        if self.brightness_bits == brightness_bits {
            return;
        }

        self.brightness_bits = brightness_bits;
        if brightness <= 0.0 {
            self.brightness_lut = [0; 256];
            return;
        }
        if brightness >= 0.999 {
            self.brightness_lut = identity_brightness_lut();
            return;
        }

        self.brightness_lut = std::array::from_fn(|channel| {
            linear_to_srgb_u8(
                srgb_u8_to_linear(
                    u8::try_from(channel)
                        .expect("preview JPEG brightness LUT indices should fit in a byte"),
                ) * brightness,
            )
        });
    }
}

#[cfg(test)]
pub(super) fn encode_canvas_jpeg_binary_stateless(
    frame: &CanvasFrame,
    header: u8,
    brightness: f32,
) -> Result<axum::body::Bytes> {
    let mut encoder = PreviewJpegEncoder::new()?;
    encoder.encode(frame, header, brightness)
}

pub(super) fn encode_canvas_jpeg_payload_scaled_stateless(
    frame: &CanvasFrame,
    header: u8,
    brightness: f32,
    requested_width: u32,
    requested_height: u32,
) -> Result<Vec<u8>> {
    let mut encoder = PreviewJpegEncoder::new()?;
    encoder.encode_scaled_payload(frame, header, brightness, requested_width, requested_height)
}

fn write_canvas_header(
    out: &mut [u8],
    header: u8,
    frame: &CanvasFrame,
    width_u16: u16,
    height_u16: u16,
    format_tag: u8,
) {
    out[0] = header;
    out[1..5].copy_from_slice(&frame.frame_number.to_le_bytes());
    out[5..9].copy_from_slice(&frame.timestamp_ms.to_le_bytes());
    out[9..11].copy_from_slice(&width_u16.to_le_bytes());
    out[11..13].copy_from_slice(&height_u16.to_le_bytes());
    out[13] = format_tag;
}

fn identity_brightness_lut() -> [u8; 256] {
    std::array::from_fn(|channel| {
        u8::try_from(channel)
            .expect("preview JPEG brightness LUT indices should remain within byte range")
    })
}

fn resolve_preview_dimensions(
    source_width: u32,
    source_height: u32,
    requested_width: u32,
    requested_height: u32,
) -> (u32, u32) {
    if source_width == 0 || source_height == 0 {
        return (source_width, source_height);
    }
    if requested_width == 0 && requested_height == 0 {
        return (source_width, source_height);
    }
    if requested_width == 0 {
        let height = requested_height.max(1);
        let width = u32::try_from(
            (u64::from(source_width) * u64::from(height))
                .checked_div(u64::from(source_height))
                .unwrap_or(1),
        )
        .unwrap_or(u32::MAX)
        .max(1);
        return (width, height);
    }
    if requested_height == 0 {
        let width = requested_width.max(1);
        let height = u32::try_from(
            (u64::from(source_height) * u64::from(width))
                .checked_div(u64::from(source_width))
                .unwrap_or(1),
        )
        .unwrap_or(u32::MAX)
        .max(1);
        return (width, height);
    }
    (requested_width.max(1), requested_height.max(1))
}
