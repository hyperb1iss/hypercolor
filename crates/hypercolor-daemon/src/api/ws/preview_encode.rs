use anyhow::{Context, Result};
use axum::body::Bytes;
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

    pub(super) fn encode(
        &mut self,
        frame: &CanvasFrame,
        header: u8,
        brightness: f32,
    ) -> Result<Bytes> {
        let mut jpeg = self.encode_body(frame, brightness)?;
        let width_u16 = u16::try_from(frame.width).unwrap_or(u16::MAX);
        let height_u16 = u16::try_from(frame.height).unwrap_or(u16::MAX);
        let mut payload = Vec::with_capacity(CANVAS_HEADER_LEN.saturating_add(jpeg.len()));
        write_canvas_header(
            &mut payload,
            header,
            frame,
            width_u16,
            height_u16,
            JPEG_FORMAT_TAG,
        );
        payload.extend_from_slice(&jpeg);
        jpeg.clear();
        self.jpeg_buffer = jpeg;
        Ok(Bytes::from(payload))
    }

    pub(super) fn encode_body(&mut self, frame: &CanvasFrame, brightness: f32) -> Result<Vec<u8>> {
        let brightness = brightness.clamp(0.0, 1.0);
        let width_u16 = u16::try_from(frame.width).unwrap_or(u16::MAX);
        let height_u16 = u16::try_from(frame.height).unwrap_or(u16::MAX);
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

        let (pixels, pixel_format) = if brightness >= 0.999 {
            (frame.rgba_bytes(), TurboJpegPixelFormat::RGBA)
        } else {
            self.refresh_brightness_lut(brightness);
            self.copy_dimmed_rgba_pixels(frame);
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

    fn copy_dimmed_rgba_pixels(&mut self, frame: &CanvasFrame) {
        let width = usize::try_from(frame.width).unwrap_or(0);
        let height = usize::try_from(frame.height).unwrap_or(0);
        let pixel_count = width.saturating_mul(height);
        let required_len = pixel_count.saturating_mul(TurboJpegPixelFormat::RGBA.size());
        self.rgba_buffer.clear();
        self.rgba_buffer.resize(required_len, 0);
        let rgba = frame.rgba_bytes();
        for (pixel, out) in rgba
            .chunks_exact(4)
            .take(pixel_count)
            .zip(self.rgba_buffer.chunks_exact_mut(4))
        {
            out[0] = self.brightness_lut[usize::from(pixel[0])];
            out[1] = self.brightness_lut[usize::from(pixel[1])];
            out[2] = self.brightness_lut[usize::from(pixel[2])];
            out[3] = pixel[3];
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

pub(super) fn encode_canvas_jpeg_binary_stateless(
    frame: &CanvasFrame,
    header: u8,
    brightness: f32,
) -> Result<Bytes> {
    let mut encoder = PreviewJpegEncoder::new()?;
    encoder.encode(frame, header, brightness)
}

fn write_canvas_header(
    out: &mut Vec<u8>,
    header: u8,
    frame: &CanvasFrame,
    width_u16: u16,
    height_u16: u16,
    format_tag: u8,
) {
    out.push(header);
    out.extend_from_slice(&frame.frame_number.to_le_bytes());
    out.extend_from_slice(&frame.timestamp_ms.to_le_bytes());
    out.extend_from_slice(&width_u16.to_le_bytes());
    out.extend_from_slice(&height_u16.to_le_bytes());
    out.push(format_tag);
}

fn identity_brightness_lut() -> [u8; 256] {
    std::array::from_fn(|channel| {
        u8::try_from(channel)
            .expect("preview JPEG brightness LUT indices should remain within byte range")
    })
}
