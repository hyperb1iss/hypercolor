use anyhow::{Context, Result};
use turbojpeg::{
    Compressor as TurboJpegCompressor, Image as TurboJpegImage,
    PixelFormat as TurboJpegPixelFormat, Subsamp as TurboJpegSubsamp,
    compressed_buf_len as turbojpeg_compressed_buf_len,
};

use hypercolor_core::bus::CanvasFrame;
use hypercolor_leptos_ext::ws::{
    PreviewFrame, PreviewFrameChannel, PreviewPixelFormat as WirePreviewPixelFormat,
};
use hypercolor_types::canvas::{linear_to_srgb_u8, srgb_u8_to_linear};

use super::preview_scale::{PreviewScaleFormat, scale_rgba_bilinear};
use super::protocol::{CanvasFormat, validate_preview_surface_resource};

const PREVIEW_JPEG_QUALITY: u8 = 80;
const PREVIEW_JPEG_SUBSAMP: TurboJpegSubsamp = TurboJpegSubsamp::Sub2x2;

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
    ) -> Result<Vec<u8>> {
        let brightness = brightness.clamp(0.0, 1.0);
        let (target_width, target_height) = resolve_preview_dimensions(
            frame.width,
            frame.height,
            requested_width,
            requested_height,
        )?;
        let out_bpp = preview_format_bytes_per_pixel(format);
        let target_len = checked_pixel_bytes(target_width, target_height, out_bpp)?;
        let mut body = std::mem::take(&mut self.body_buffer);
        resize_buffer(&mut body, target_len)?;

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
                    copy_rgba_to_rgb(frame.rgba_bytes(), brightness_lut, &mut body)?;
                }
                CanvasFormat::Rgba => {
                    if brightness_lut.is_some() {
                        copy_rgba_to_rgba(frame.rgba_bytes(), brightness_lut, &mut body)?;
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
            )?;
        }

        Ok(body)
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
        )?;
        let jpeg = self.encode_scaled_body(frame, brightness, requested_width, requested_height)?;
        let channel = PreviewFrameChannel::try_from(header)
            .context("preview header is not a passive preview channel")?;
        PreviewFrame {
            channel,
            frame_number: frame.frame_number,
            timestamp_ms: frame.timestamp_ms,
            width: target_width,
            height: target_height,
            format: WirePreviewPixelFormat::Jpeg,
            payload: jpeg.into(),
        }
        .try_encode()
        .context("failed to encode preview JPEG wire frame")
        .map(|bytes| bytes.to_vec())
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
        )?;
        let width = usize::try_from(target_width).context("preview width exceeds address space")?;
        let height =
            usize::try_from(target_height).context("preview height exceeds address space")?;
        let required_len = turbojpeg_compressed_buf_len(width, height, PREVIEW_JPEG_SUBSAMP)
            .context("failed to size preview JPEG buffer")?;
        let mut jpeg_buffer = std::mem::take(&mut self.jpeg_buffer);
        resize_buffer(&mut jpeg_buffer, required_len)?;

        let (pixels, pixel_format) = if brightness >= 0.999
            && target_width == frame.width
            && target_height == frame.height
        {
            (frame.rgba_bytes(), TurboJpegPixelFormat::RGBA)
        } else {
            self.refresh_brightness_lut(brightness);
            self.copy_scaled_rgb_pixels(frame, target_width, target_height, brightness)?;
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
    ) -> Result<()> {
        let brightness_lut = (brightness < 0.999).then_some(&self.brightness_lut);
        if target_width == frame.width && target_height == frame.height {
            copy_rgba_to_rgb(frame.rgba_bytes(), brightness_lut, &mut self.rgb_buffer)?;
            return Ok(());
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
        )
    }

    fn refresh_brightness_lut(&mut self, brightness: f32) {
        refresh_brightness_lut(
            brightness,
            &mut self.brightness_bits,
            &mut self.brightness_lut,
        );
    }
}

fn copy_rgba_to_rgb(
    rgba: &[u8],
    brightness_lut: Option<&[u8; 256]>,
    out: &mut Vec<u8>,
) -> Result<()> {
    let required_len = (rgba.len() / 4)
        .checked_mul(3)
        .context("preview RGB byte length overflow")?;
    resize_buffer(out, required_len)?;

    if let Some(brightness_lut) = brightness_lut {
        copy_rgba_to_rgb_with_lut(rgba, brightness_lut, out);
    } else {
        copy_rgba_to_rgb_raw(rgba, out);
    }
    Ok(())
}

fn copy_rgba_to_rgba(
    rgba: &[u8],
    brightness_lut: Option<&[u8; 256]>,
    out: &mut Vec<u8>,
) -> Result<()> {
    resize_buffer(out, rgba.len())?;

    if let Some(brightness_lut) = brightness_lut {
        copy_rgba_to_rgba_with_lut(rgba, brightness_lut, out);
    } else {
        out.copy_from_slice(rgba);
    }
    Ok(())
}

fn copy_rgba_to_rgb_raw(rgba: &[u8], out: &mut [u8]) {
    let mut src = 0;
    let mut dst = 0;
    let unrolled_src_len = rgba.len() / 16 * 16;

    while src < unrolled_src_len {
        out[dst] = rgba[src];
        out[dst + 1] = rgba[src + 1];
        out[dst + 2] = rgba[src + 2];
        out[dst + 3] = rgba[src + 4];
        out[dst + 4] = rgba[src + 5];
        out[dst + 5] = rgba[src + 6];
        out[dst + 6] = rgba[src + 8];
        out[dst + 7] = rgba[src + 9];
        out[dst + 8] = rgba[src + 10];
        out[dst + 9] = rgba[src + 12];
        out[dst + 10] = rgba[src + 13];
        out[dst + 11] = rgba[src + 14];
        src += 16;
        dst += 12;
    }

    while src + 3 < rgba.len() {
        out[dst] = rgba[src];
        out[dst + 1] = rgba[src + 1];
        out[dst + 2] = rgba[src + 2];
        src += 4;
        dst += 3;
    }
}

fn copy_rgba_to_rgb_with_lut(rgba: &[u8], brightness_lut: &[u8; 256], out: &mut [u8]) {
    let mut src = 0;
    let mut dst = 0;

    while src + 3 < rgba.len() {
        out[dst] = brightness_lut[usize::from(rgba[src])];
        out[dst + 1] = brightness_lut[usize::from(rgba[src + 1])];
        out[dst + 2] = brightness_lut[usize::from(rgba[src + 2])];
        src += 4;
        dst += 3;
    }
}

fn copy_rgba_to_rgba_with_lut(rgba: &[u8], brightness_lut: &[u8; 256], out: &mut [u8]) {
    let mut offset = 0;

    while offset + 3 < rgba.len() {
        out[offset] = brightness_lut[usize::from(rgba[offset])];
        out[offset + 1] = brightness_lut[usize::from(rgba[offset + 1])];
        out[offset + 2] = brightness_lut[usize::from(rgba[offset + 2])];
        out[offset + 3] = rgba[offset + 3];
        offset += 4;
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
) -> Result<(u32, u32)> {
    let dimensions = if source_width == 0 || source_height == 0 {
        (source_width, source_height)
    } else if requested_width == 0 && requested_height == 0 {
        (source_width, source_height)
    } else if requested_width == 0 {
        let height = requested_height.max(1);
        let width = u32::try_from(
            (u64::from(source_width) * u64::from(height))
                .checked_div(u64::from(source_height))
                .context("preview aspect division failed")?,
        )
        .context("preview aspect width exceeds u32")?
        .max(1);
        (width, height)
    } else if requested_height == 0 {
        let width = requested_width.max(1);
        let height = u32::try_from(
            (u64::from(source_height) * u64::from(width))
                .checked_div(u64::from(source_width))
                .context("preview aspect division failed")?,
        )
        .context("preview aspect height exceeds u32")?
        .max(1);
        (width, height)
    } else {
        (requested_width.max(1), requested_height.max(1))
    };
    validate_preview_surface_resource(dimensions.0, dimensions.1).map_err(anyhow::Error::msg)?;
    Ok(dimensions)
}

fn checked_pixel_bytes(width: u32, height: u32, bytes_per_pixel: usize) -> Result<usize> {
    usize::try_from(width)
        .context("preview width exceeds platform address space")?
        .checked_mul(
            usize::try_from(height).context("preview height exceeds platform address space")?,
        )
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .context("preview byte length overflow")
}

fn resize_buffer(buffer: &mut Vec<u8>, required_len: usize) -> Result<()> {
    if required_len > buffer.len() {
        buffer
            .try_reserve_exact(required_len - buffer.len())
            .with_context(|| format!("failed to reserve {required_len} preview bytes"))?;
    }
    buffer.resize(required_len, 0);
    Ok(())
}
