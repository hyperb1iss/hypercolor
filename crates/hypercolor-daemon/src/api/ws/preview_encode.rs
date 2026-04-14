use anyhow::{Context, Result};
use turbojpeg::{
    Compressor as TurboJpegCompressor, Image as TurboJpegImage,
    PixelFormat as TurboJpegPixelFormat, Subsamp as TurboJpegSubsamp,
    compressed_buf_len as turbojpeg_compressed_buf_len,
};

use hypercolor_core::bus::CanvasFrame;
use hypercolor_types::canvas::{linear_to_srgb_u8, srgb_u8_to_linear};

use super::preview_scale::{PreviewScaleFormat, scale_rgba_bilinear};
use super::protocol::CanvasFormat;

const CANVAS_HEADER_LEN: usize = 14;
const PREVIEW_JPEG_QUALITY: u8 = 80;
const PREVIEW_JPEG_SUBSAMP: TurboJpegSubsamp = TurboJpegSubsamp::Sub2x2;
const JPEG_FORMAT_TAG: u8 = 2;

pub(super) struct PreviewRawEncoder {
    body_buffer: Vec<u8>,
    brightness_bits: u32,
    brightness_lut: [u8; 256],
}

impl PreviewRawEncoder {
    pub(super) fn new() -> Self {
        Self {
            body_buffer: Vec::new(),
            brightness_bits: 1.0_f32.to_bits(),
            brightness_lut: identity_brightness_lut(),
        }
    }

    pub(super) fn encode_scaled_body(
        &mut self,
        frame: &CanvasFrame,
        format: CanvasFormat,
        brightness: f32,
        requested_width: u32,
        requested_height: u32,
    ) -> Vec<u8> {
        let brightness = brightness.clamp(0.0, 1.0);
        let (target_width, target_height) = resolve_preview_dimensions(
            frame.width,
            frame.height,
            requested_width,
            requested_height,
        );
        let out_bpp = preview_format_bytes_per_pixel(format);
        let target_len = usize::try_from(target_width)
            .unwrap_or(0)
            .saturating_mul(usize::try_from(target_height).unwrap_or(0))
            .saturating_mul(out_bpp);
        let mut body = std::mem::take(&mut self.body_buffer);
        if body.len() != target_len {
            body.resize(target_len, 0);
        }

        let brightness_lut = if brightness < 0.999 {
            refresh_brightness_lut(
                brightness,
                &mut self.brightness_bits,
                &mut self.brightness_lut,
            );
            Some(&self.brightness_lut)
        } else {
            None
        };

        if target_width == frame.width && target_height == frame.height {
            match format {
                CanvasFormat::Rgb => {
                    copy_rgba_to_rgb(frame.rgba_bytes(), brightness_lut, &mut body);
                }
                CanvasFormat::Rgba => {
                    if brightness_lut.is_some() {
                        copy_rgba_to_rgba(frame.rgba_bytes(), brightness_lut, &mut body);
                    } else {
                        body.copy_from_slice(frame.rgba_bytes());
                    }
                }
                CanvasFormat::Jpeg => unreachable!("JPEG preview bodies use the JPEG encoder"),
            }
        } else {
            scale_rgba_bilinear(
                frame.rgba_bytes(),
                frame.width,
                frame.height,
                target_width,
                target_height,
                brightness_lut,
                preview_scale_format(format),
                &mut body,
            );
        }

        body
    }
}

pub(super) struct PreviewJpegEncoder {
    rgb_buffer: Vec<u8>,
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
            rgb_buffer: Vec::new(),
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
        let (target_width, target_height) = resolve_preview_dimensions(
            frame.width,
            frame.height,
            requested_width,
            requested_height,
        );
        let mut jpeg =
            self.encode_scaled_body(frame, brightness, requested_width, requested_height)?;
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
        let (target_width, target_height) = resolve_preview_dimensions(
            frame.width,
            frame.height,
            requested_width,
            requested_height,
        );
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
            self.copy_scaled_rgb_pixels(frame, target_width, target_height, brightness);
            (self.rgb_buffer.as_slice(), TurboJpegPixelFormat::RGB)
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

    fn copy_scaled_rgb_pixels(
        &mut self,
        frame: &CanvasFrame,
        target_width: u32,
        target_height: u32,
        brightness: f32,
    ) {
        let brightness_lut = (brightness < 0.999).then_some(&self.brightness_lut);
        if target_width == frame.width && target_height == frame.height {
            copy_rgba_to_rgb(frame.rgba_bytes(), brightness_lut, &mut self.rgb_buffer);
            return;
        }

        scale_rgba_bilinear(
            frame.rgba_bytes(),
            frame.width,
            frame.height,
            target_width,
            target_height,
            brightness_lut,
            PreviewScaleFormat::Rgb,
            &mut self.rgb_buffer,
        );
    }

    fn refresh_brightness_lut(&mut self, brightness: f32) {
        refresh_brightness_lut(
            brightness,
            &mut self.brightness_bits,
            &mut self.brightness_lut,
        );
    }
}

fn copy_rgba_to_rgb(rgba: &[u8], brightness_lut: Option<&[u8; 256]>, out: &mut Vec<u8>) {
    let required_len = rgba.len() / 4 * 3;
    if out.len() != required_len {
        out.resize(required_len, 0);
    }

    for (pixel, out_pixel) in rgba.chunks_exact(4).zip(out.chunks_exact_mut(3)) {
        if let Some(brightness_lut) = brightness_lut {
            out_pixel[0] = brightness_lut[usize::from(pixel[0])];
            out_pixel[1] = brightness_lut[usize::from(pixel[1])];
            out_pixel[2] = brightness_lut[usize::from(pixel[2])];
        } else {
            out_pixel.copy_from_slice(&pixel[..3]);
        }
    }
}

fn copy_rgba_to_rgba(rgba: &[u8], brightness_lut: Option<&[u8; 256]>, out: &mut Vec<u8>) {
    if out.len() != rgba.len() {
        out.resize(rgba.len(), 0);
    }

    if let Some(brightness_lut) = brightness_lut {
        for (pixel, out_pixel) in rgba.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
            out_pixel[0] = brightness_lut[usize::from(pixel[0])];
            out_pixel[1] = brightness_lut[usize::from(pixel[1])];
            out_pixel[2] = brightness_lut[usize::from(pixel[2])];
            out_pixel[3] = pixel[3];
        }
    } else {
        out.copy_from_slice(rgba);
    }
}

fn preview_scale_format(format: CanvasFormat) -> PreviewScaleFormat {
    match format {
        CanvasFormat::Rgb => PreviewScaleFormat::Rgb,
        CanvasFormat::Rgba => PreviewScaleFormat::Rgba,
        CanvasFormat::Jpeg => unreachable!("JPEG previews use the JPEG encoder"),
    }
}

fn preview_format_bytes_per_pixel(format: CanvasFormat) -> usize {
    match format {
        CanvasFormat::Rgb => 3,
        CanvasFormat::Rgba => 4,
        CanvasFormat::Jpeg => unreachable!("JPEG previews use the JPEG encoder"),
    }
}

fn refresh_brightness_lut(
    brightness: f32,
    brightness_bits: &mut u32,
    brightness_lut: &mut [u8; 256],
) {
    let next_brightness_bits = brightness.to_bits();
    if *brightness_bits == next_brightness_bits {
        return;
    }

    *brightness_bits = next_brightness_bits;
    if brightness <= 0.0 {
        *brightness_lut = [0; 256];
        return;
    }
    if brightness >= 0.999 {
        *brightness_lut = identity_brightness_lut();
        return;
    }

    *brightness_lut = std::array::from_fn(|channel| {
        linear_to_srgb_u8(
            srgb_u8_to_linear(
                u8::try_from(channel).expect("preview brightness LUT indices should fit in a byte"),
            ) * brightness,
        )
    });
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
