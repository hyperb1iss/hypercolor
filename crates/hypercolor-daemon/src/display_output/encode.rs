//! JPEG encoding and brightness scaling for display frames.

use anyhow::{Context, Result};
use turbojpeg::{
    Compressor as TurboJpegCompressor, Image as TurboJpegImage,
    PixelFormat as TurboJpegPixelFormat, Subsamp as TurboJpegSubsamp,
    compressed_buf_len as turbojpeg_compressed_buf_len,
};

use hypercolor_core::bus::CanvasFrame;

use super::render::{
    PreparedDisplayPlan, apply_circular_mask, render_display_view, rgb_buffer_len,
};
use super::{DisplayGeometry, DisplayViewport};

const JPEG_QUALITY: u8 = 85;
const JPEG_SUBSAMP: TurboJpegSubsamp = TurboJpegSubsamp::Sub2x2;

pub(super) struct DisplayEncodeState {
    pub rgb_buffer: Vec<u8>,
    pub jpeg_buffer: Vec<u8>,
    pub jpeg_compressor: TurboJpegCompressor,
    pub axis_plan: Option<PreparedDisplayPlan>,
    brightness_factor: u16,
    brightness_lut: [u8; 256],
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
            brightness_factor: u16::from(u8::MAX),
            brightness_lut: identity_brightness_lut(),
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
    let brightness_factor = display_brightness_factor(brightness);
    if brightness_factor == 0 {
        prepare_black_frame(geometry, &mut encode_state.rgb_buffer);
        return encode_rgb_to_jpeg(geometry, encode_state);
    }

    let brightness_lut = if brightness_factor >= u16::from(u8::MAX) {
        None
    } else {
        refresh_display_brightness_lut(encode_state, brightness_factor);
        Some(&encode_state.brightness_lut)
    };

    if geometry.width == 0 || geometry.height == 0 {
        encode_state.rgb_buffer.clear();
    } else {
        render_display_view(
            source,
            viewport,
            geometry.width,
            geometry.height,
            &mut encode_state.rgb_buffer,
            &mut encode_state.axis_plan,
            brightness_lut,
        );
    }

    if geometry.circular {
        apply_circular_mask(
            &mut encode_state.rgb_buffer,
            geometry.width,
            geometry.height,
        );
    }

    encode_rgb_to_jpeg(geometry, encode_state)
}

pub(super) fn render_canvas_frame_rgb(
    source: &CanvasFrame,
    viewport: &DisplayViewport,
    geometry: &DisplayGeometry,
    encode_state: &mut DisplayEncodeState,
) {
    if geometry.width == 0 || geometry.height == 0 {
        encode_state.rgb_buffer.clear();
        return;
    }

    render_display_view(
        source,
        viewport,
        geometry.width,
        geometry.height,
        &mut encode_state.rgb_buffer,
        &mut encode_state.axis_plan,
        None,
    );
}

pub(super) fn encode_prepared_rgb_frame(
    geometry: &DisplayGeometry,
    brightness: f32,
    encode_state: &mut DisplayEncodeState,
) -> Result<Vec<u8>> {
    let brightness_factor = display_brightness_factor(brightness);
    if brightness_factor == 0 {
        prepare_black_frame(geometry, &mut encode_state.rgb_buffer);
        return encode_rgb_to_jpeg(geometry, encode_state);
    }

    refresh_display_brightness_lut(encode_state, brightness_factor);
    apply_display_brightness(
        &mut encode_state.rgb_buffer,
        brightness_factor,
        &encode_state.brightness_lut,
    );
    if geometry.circular {
        apply_circular_mask(
            &mut encode_state.rgb_buffer,
            geometry.width,
            geometry.height,
        );
    }

    encode_rgb_to_jpeg(geometry, encode_state)
}

pub(super) fn apply_display_brightness(
    rgb_buffer: &mut [u8],
    brightness_factor: u16,
    brightness_lut: &[u8; 256],
) {
    if brightness_factor >= u16::from(u8::MAX) {
        return;
    }
    for channel in rgb_buffer {
        *channel = brightness_lut[usize::from(*channel)];
    }
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

fn scale_channel(channel: u8, factor: u16) -> u8 {
    let scaled = (u16::from(channel) * factor) / u16::from(u8::MAX);
    u8::try_from(scaled).expect("display brightness scaling should remain within byte range")
}

fn refresh_display_brightness_lut(encode_state: &mut DisplayEncodeState, brightness_factor: u16) {
    if encode_state.brightness_factor != brightness_factor {
        encode_state.brightness_factor = brightness_factor;
        encode_state.brightness_lut = std::array::from_fn(|channel| {
            scale_channel(
                u8::try_from(channel)
                    .expect("brightness lookup indices should remain within byte range"),
                brightness_factor,
            )
        });
    }
}

fn prepare_black_frame(geometry: &DisplayGeometry, rgb_buffer: &mut Vec<u8>) {
    let Some(render_len) = rgb_buffer_len(geometry.width, geometry.height) else {
        rgb_buffer.clear();
        return;
    };

    if rgb_buffer.len() != render_len {
        rgb_buffer.resize(render_len, 0);
    } else {
        rgb_buffer.fill(0);
    }
}

fn identity_brightness_lut() -> [u8; 256] {
    std::array::from_fn(|channel| {
        u8::try_from(channel).expect("brightness lookup indices should remain within byte range")
    })
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
