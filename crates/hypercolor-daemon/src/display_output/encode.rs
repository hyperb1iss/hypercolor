//! JPEG encoding and brightness scaling for display frames.

use anyhow::{Context, Result};
use turbojpeg::{
    Compressor as TurboJpegCompressor, Image as TurboJpegImage,
    PixelFormat as TurboJpegPixelFormat, Subsamp as TurboJpegSubsamp,
    compressed_buf_len as turbojpeg_compressed_buf_len,
};

use hypercolor_core::bus::CanvasFrame;

use super::render::{apply_circular_mask, render_display_view, PreparedDisplayPlan};
use super::{DisplayGeometry, DisplayViewport};

const JPEG_QUALITY: u8 = 85;
const JPEG_SUBSAMP: TurboJpegSubsamp = TurboJpegSubsamp::Sub2x2;

pub(super) struct DisplayEncodeState {
    pub rgb_buffer: Vec<u8>,
    pub jpeg_buffer: Vec<u8>,
    pub jpeg_compressor: TurboJpegCompressor,
    pub axis_plan: Option<PreparedDisplayPlan>,
}

impl DisplayEncodeState {
    pub fn new() -> Result<Self> {
        let mut jpeg_compressor =
            TurboJpegCompressor::new().context("failed to initialize TurboJPEG display encoder")?;
        jpeg_compressor
            .set_quality(i32::from(JPEG_QUALITY))
            .context("failed to configure TurboJPEG quality")?;
        jpeg_compressor
            .set_subsamp(JPEG_SUBSAMP)
            .context("failed to configure TurboJPEG chroma subsampling")?;

        Ok(Self {
            rgb_buffer: Vec::new(),
            jpeg_buffer: Vec::new(),
            jpeg_compressor,
            axis_plan: None,
        })
    }
}

pub(super) fn encode_canvas_frame(
    source: &CanvasFrame,
    viewport: &DisplayViewport,
    geometry: &DisplayGeometry,
    brightness: f32,
    encode_state: &mut DisplayEncodeState,
) -> Result<Vec<u8>> {
    render_display_view(
        source,
        viewport,
        geometry.width,
        geometry.height,
        &mut encode_state.rgb_buffer,
        &mut encode_state.axis_plan,
    );
    apply_display_brightness(&mut encode_state.rgb_buffer, brightness);
    if geometry.circular {
        apply_circular_mask(
            &mut encode_state.rgb_buffer,
            geometry.width,
            geometry.height,
        );
    }

    encode_rgb_to_jpeg(geometry, encode_state)
}

fn encode_rgb_to_jpeg(
    geometry: &DisplayGeometry,
    encode_state: &mut DisplayEncodeState,
) -> Result<Vec<u8>> {
    let width = usize::try_from(geometry.width).context("display width does not fit usize")?;
    let height = usize::try_from(geometry.height).context("display height does not fit usize")?;
    let pitch = width
        .checked_mul(TurboJpegPixelFormat::RGB.size())
        .context("display row pitch overflow")?;
    let required_len = turbojpeg_compressed_buf_len(width, height, JPEG_SUBSAMP)
        .context("failed to size TurboJPEG display buffer")?;

    let mut jpeg_buffer = std::mem::take(&mut encode_state.jpeg_buffer);
    if jpeg_buffer.len() < required_len {
        jpeg_buffer.resize(required_len, 0);
    } else {
        jpeg_buffer.truncate(required_len);
    }

    let image = TurboJpegImage {
        pixels: encode_state.rgb_buffer.as_slice(),
        width,
        pitch,
        height,
        format: TurboJpegPixelFormat::RGB,
    };
    let jpeg_len = match encode_state
        .jpeg_compressor
        .compress_to_slice(image, jpeg_buffer.as_mut_slice())
    {
        Ok(len) => len,
        Err(error) => {
            encode_state.jpeg_buffer = jpeg_buffer;
            return Err(error).context("failed to TurboJPEG-encode display frame");
        }
    };

    jpeg_buffer.truncate(jpeg_len);
    Ok(jpeg_buffer)
}

fn apply_display_brightness(image: &mut [u8], brightness: f32) {
    let factor = display_brightness_factor(brightness);
    if factor >= u16::from(u8::MAX) {
        return;
    }
    if factor == 0 {
        image.fill(0);
        return;
    }

    for pixel in image.chunks_exact_mut(3) {
        pixel[0] = scale_channel(pixel[0], factor);
        pixel[1] = scale_channel(pixel[1], factor);
        pixel[2] = scale_channel(pixel[2], factor);
    }
}

fn scale_channel(channel: u8, factor: u16) -> u8 {
    let scaled = (u16::from(channel) * factor) / u16::from(u8::MAX);
    u8::try_from(scaled).expect("display brightness scaling should remain within byte range")
}

pub(super) fn display_brightness_factor(brightness: f32) -> u16 {
    round_unit_to_u16(brightness.clamp(0.0, 1.0))
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the helper bounds finite unit values to the 0-255 brightness factor range"
)]
fn round_unit_to_u16(value: f32) -> u16 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    if value >= 1.0 {
        return u16::from(u8::MAX);
    }

    ((value * f32::from(u8::MAX)) + 0.5) as u16
}
