//! JPEG encoding and brightness scaling for display frames.

use anyhow::{Context, Result};
use fast_image_resize as fr;
use tracing::debug;
use turbojpeg::{
    Compressor as TurboJpegCompressor, Image as TurboJpegImage,
    PixelFormat as TurboJpegPixelFormat, Subsamp as TurboJpegSubsamp,
    compressed_buf_len as turbojpeg_compressed_buf_len,
};

use hypercolor_core::bus::CanvasFrame;

use super::render::{
    PreparedDisplayPlan, apply_circular_mask, apply_circular_mask_rgba, fast_display_crop,
    render_display_view, rgb_buffer_len, rgba_buffer_len,
};
use super::{DisplayGeometry, DisplayViewport};

const JPEG_QUALITY: u8 = 85;
const JPEG_SUBSAMP: TurboJpegSubsamp = TurboJpegSubsamp::Sub2x2;

pub(super) struct DisplayEncodeState {
    pub rgb_buffer: Vec<u8>,
    pub rgba_buffer: Vec<u8>,
    pub scratch_rgba_buffer: Vec<u8>,
    pub jpeg_buffer: Vec<u8>,
    pub jpeg_compressor: TurboJpegCompressor,
    pub fast_resizer: fr::Resizer,
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
            rgba_buffer: Vec::new(),
            scratch_rgba_buffer: Vec::new(),
            jpeg_buffer: Vec::new(),
            jpeg_compressor,
            fast_resizer: fr::Resizer::new(),
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

    let use_brightness_lut = brightness_factor < u16::from(u8::MAX);
    if use_brightness_lut {
        refresh_display_brightness_lut(encode_state, brightness_factor);
    }

    let used_fast_path = if geometry.width == 0 || geometry.height == 0 {
        encode_state.rgb_buffer.clear();
        encode_state.rgba_buffer.clear();
        false
    } else {
        match try_render_canvas_frame_rgba_fast(source, viewport, geometry, encode_state, false) {
            Ok(true) => true,
            Ok(false) => {
                render_display_view(
                    source,
                    viewport,
                    geometry.width,
                    geometry.height,
                    &mut encode_state.rgb_buffer,
                    &mut encode_state.axis_plan,
                    use_brightness_lut.then_some(&encode_state.brightness_lut),
                );
                false
            }
            Err(error) => {
                debug!(%error, "fast display resize fell back to scalar path");
                render_display_view(
                    source,
                    viewport,
                    geometry.width,
                    geometry.height,
                    &mut encode_state.rgb_buffer,
                    &mut encode_state.axis_plan,
                    use_brightness_lut.then_some(&encode_state.brightness_lut),
                );
                false
            }
        }
    };

    if used_fast_path && use_brightness_lut {
        apply_display_brightness_rgba(&mut encode_state.rgba_buffer, &encode_state.brightness_lut);
    }

    if geometry.circular {
        if used_fast_path {
            apply_circular_mask_rgba(
                &mut encode_state.rgba_buffer,
                geometry.width,
                geometry.height,
            );
        } else {
            apply_circular_mask(
                &mut encode_state.rgb_buffer,
                geometry.width,
                geometry.height,
            );
        }
    }

    if used_fast_path {
        encode_rgba_to_jpeg(geometry, encode_state)
    } else {
        encode_rgb_to_jpeg(geometry, encode_state)
    }
}

pub(super) fn encode_direct_canvas_frame(
    source: &CanvasFrame,
    geometry: &DisplayGeometry,
    brightness: f32,
    encode_state: &mut DisplayEncodeState,
) -> Result<Vec<u8>> {
    render_direct_canvas_frame_rgb(source, geometry, encode_state);
    encode_prepared_rgb_frame(geometry, brightness, encode_state)
}

pub(super) fn encode_face_effect_blend(
    effect_source: Option<&CanvasFrame>,
    face_source: &CanvasFrame,
    viewport: &DisplayViewport,
    geometry: &DisplayGeometry,
    brightness: f32,
    face_opacity: f32,
    encode_state: &mut DisplayEncodeState,
) -> Result<Vec<u8>> {
    if let Some(effect_source) = effect_source {
        render_canvas_frame_rgba(effect_source, viewport, geometry, encode_state, false)?;
    } else {
        prepare_black_rgba_frame(geometry, &mut encode_state.rgba_buffer);
    }

    render_direct_canvas_frame_rgba(face_source, geometry, encode_state, true)?;
    blend_face_rgba_over_opaque_rgba(
        &mut encode_state.rgba_buffer,
        &encode_state.scratch_rgba_buffer,
        face_opacity,
    );
    encode_prepared_rgba_frame(geometry, brightness, encode_state)
}

pub(super) fn render_direct_canvas_frame_rgb(
    source: &CanvasFrame,
    geometry: &DisplayGeometry,
    encode_state: &mut DisplayEncodeState,
) {
    if geometry.width == 0 || geometry.height == 0 {
        encode_state.rgb_buffer.clear();
        return;
    }

    if source.width == geometry.width && source.height == geometry.height {
        let Some(render_len) = rgb_buffer_len(geometry.width, geometry.height) else {
            encode_state.rgb_buffer.clear();
            return;
        };
        if encode_state.rgb_buffer.len() != render_len {
            encode_state.rgb_buffer.resize(render_len, 0);
        }

        for (pixel, rgba) in encode_state
            .rgb_buffer
            .chunks_exact_mut(3)
            .zip(source.rgba_bytes().chunks_exact(4))
        {
            pixel[0] = rgba[0];
            pixel[1] = rgba[1];
            pixel[2] = rgba[2];
        }
        return;
    }

    render_display_view(
        source,
        &DisplayViewport {
            position: hypercolor_types::spatial::NormalizedPosition::new(0.5, 0.5),
            size: hypercolor_types::spatial::NormalizedPosition::new(1.0, 1.0),
            rotation: 0.0,
            scale: 1.0,
            edge_behavior: hypercolor_types::spatial::EdgeBehavior::Clamp,
        },
        geometry.width,
        geometry.height,
        &mut encode_state.rgb_buffer,
        &mut encode_state.axis_plan,
        None,
    );
}

fn render_canvas_frame_rgba(
    source: &CanvasFrame,
    viewport: &DisplayViewport,
    geometry: &DisplayGeometry,
    encode_state: &mut DisplayEncodeState,
    use_scratch: bool,
) -> Result<()> {
    if geometry.width == 0 || geometry.height == 0 {
        if use_scratch {
            encode_state.scratch_rgba_buffer.clear();
        } else {
            encode_state.rgba_buffer.clear();
        }
        return Ok(());
    }

    if try_render_canvas_frame_rgba_fast(source, viewport, geometry, encode_state, use_scratch)? {
        return Ok(());
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
    if use_scratch {
        promote_rgb_to_rgba(
            &encode_state.rgb_buffer,
            &mut encode_state.scratch_rgba_buffer,
            geometry.width,
            geometry.height,
        );
    } else {
        promote_rgb_to_rgba(
            &encode_state.rgb_buffer,
            &mut encode_state.rgba_buffer,
            geometry.width,
            geometry.height,
        );
    }
    Ok(())
}

fn render_direct_canvas_frame_rgba(
    source: &CanvasFrame,
    geometry: &DisplayGeometry,
    encode_state: &mut DisplayEncodeState,
    use_scratch: bool,
) -> Result<()> {
    if geometry.width == 0 || geometry.height == 0 {
        if use_scratch {
            encode_state.scratch_rgba_buffer.clear();
        } else {
            encode_state.rgba_buffer.clear();
        }
        return Ok(());
    }

    if source.width == geometry.width && source.height == geometry.height {
        let Some(render_len) = rgba_buffer_len(geometry.width, geometry.height) else {
            if use_scratch {
                encode_state.scratch_rgba_buffer.clear();
            } else {
                encode_state.rgba_buffer.clear();
            }
            return Ok(());
        };
        let target_buffer = if use_scratch {
            &mut encode_state.scratch_rgba_buffer
        } else {
            &mut encode_state.rgba_buffer
        };
        if target_buffer.len() != render_len {
            target_buffer.resize(render_len, 0);
        }
        target_buffer.copy_from_slice(source.rgba_bytes());
        return Ok(());
    }

    render_canvas_frame_rgba(
        source,
        &DisplayViewport {
            position: hypercolor_types::spatial::NormalizedPosition::new(0.5, 0.5),
            size: hypercolor_types::spatial::NormalizedPosition::new(1.0, 1.0),
            rotation: 0.0,
            scale: 1.0,
            edge_behavior: hypercolor_types::spatial::EdgeBehavior::Clamp,
        },
        geometry,
        encode_state,
        use_scratch,
    )
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

pub(super) fn encode_prepared_rgba_frame(
    geometry: &DisplayGeometry,
    brightness: f32,
    encode_state: &mut DisplayEncodeState,
) -> Result<Vec<u8>> {
    let brightness_factor = display_brightness_factor(brightness);
    if brightness_factor == 0 {
        prepare_black_rgba_frame(geometry, &mut encode_state.rgba_buffer);
        return encode_rgba_to_jpeg(geometry, encode_state);
    }

    refresh_display_brightness_lut(encode_state, brightness_factor);
    apply_display_brightness_rgba(&mut encode_state.rgba_buffer, &encode_state.brightness_lut);
    if geometry.circular {
        apply_circular_mask_rgba(
            &mut encode_state.rgba_buffer,
            geometry.width,
            geometry.height,
        );
    }

    encode_rgba_to_jpeg(geometry, encode_state)
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

fn apply_display_brightness_rgba(rgba_buffer: &mut [u8], brightness_lut: &[u8; 256]) {
    for pixel in rgba_buffer.chunks_exact_mut(4) {
        pixel[0] = brightness_lut[usize::from(pixel[0])];
        pixel[1] = brightness_lut[usize::from(pixel[1])];
        pixel[2] = brightness_lut[usize::from(pixel[2])];
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

fn encode_rgba_to_jpeg(
    geometry: &DisplayGeometry,
    encode_state: &mut DisplayEncodeState,
) -> Result<Vec<u8>> {
    let width = usize::try_from(geometry.width).context("display width does not fit usize")?;
    let height = usize::try_from(geometry.height).context("display height does not fit usize")?;
    let pitch = width
        .checked_mul(TurboJpegPixelFormat::RGBA.size())
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
        pixels: encode_state.rgba_buffer.as_slice(),
        width,
        pitch,
        height,
        format: TurboJpegPixelFormat::RGBA,
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

fn try_render_canvas_frame_rgba_fast(
    source: &CanvasFrame,
    viewport: &DisplayViewport,
    geometry: &DisplayGeometry,
    encode_state: &mut DisplayEncodeState,
    use_scratch: bool,
) -> Result<bool> {
    if source.width == 0 || source.height == 0 {
        if use_scratch {
            encode_state.scratch_rgba_buffer.clear();
        } else {
            encode_state.rgba_buffer.clear();
        }
        return Ok(false);
    }

    let Some(crop) = fast_display_crop(source, viewport) else {
        return Ok(false);
    };
    let Some(render_len) = rgba_buffer_len(geometry.width, geometry.height) else {
        if use_scratch {
            encode_state.scratch_rgba_buffer.clear();
        } else {
            encode_state.rgba_buffer.clear();
        }
        return Ok(false);
    };
    let target_buffer = if use_scratch {
        &mut encode_state.scratch_rgba_buffer
    } else {
        &mut encode_state.rgba_buffer
    };
    if target_buffer.len() != render_len {
        target_buffer.resize(render_len, 0);
    }

    let src_image = fr::images::ImageRef::new(
        source.width,
        source.height,
        source.rgba_bytes(),
        fr::PixelType::U8x4,
    )
    .context("display source image buffer is invalid for fast resize")?;
    let mut dst_image = fr::images::Image::from_slice_u8(
        geometry.width,
        geometry.height,
        target_buffer.as_mut_slice(),
        fr::PixelType::U8x4,
    )
    .context("display destination image buffer is invalid for fast resize")?;
    let options = fr::ResizeOptions::new()
        .resize_alg(fr::ResizeAlg::Interpolation(fr::FilterType::Bilinear))
        .crop(crop.left, crop.top, crop.width, crop.height);
    encode_state
        .fast_resizer
        .resize(&src_image, &mut dst_image, &options)
        .context("fast display resize failed")?;

    Ok(true)
}

fn promote_rgb_to_rgba(
    rgb_buffer: &[u8],
    rgba_buffer: &mut Vec<u8>,
    width: u32,
    height: u32,
) {
    let Some(render_len) = rgba_buffer_len(width, height) else {
        rgba_buffer.clear();
        return;
    };
    if rgba_buffer.len() != render_len {
        rgba_buffer.resize(render_len, 0);
    }

    for (rgba, rgb) in rgba_buffer.chunks_exact_mut(4).zip(rgb_buffer.chunks_exact(3)) {
        rgba[0] = rgb[0];
        rgba[1] = rgb[1];
        rgba[2] = rgb[2];
        rgba[3] = u8::MAX;
    }
}

fn prepare_black_rgba_frame(geometry: &DisplayGeometry, rgba_buffer: &mut Vec<u8>) {
    let Some(render_len) = rgba_buffer_len(geometry.width, geometry.height) else {
        rgba_buffer.clear();
        return;
    };
    if rgba_buffer.len() != render_len {
        rgba_buffer.resize(render_len, 0);
    }

    for pixel in rgba_buffer.chunks_exact_mut(4) {
        pixel[0] = 0;
        pixel[1] = 0;
        pixel[2] = 0;
        pixel[3] = u8::MAX;
    }
}

fn blend_face_rgba_over_opaque_rgba(
    target_rgba: &mut [u8],
    source_rgba: &[u8],
    opacity: f32,
) {
    let opacity_weight = round_unit_to_u16(opacity.clamp(0.0, 1.0));
    if opacity_weight == 0 {
        return;
    }

    for (dst, src) in target_rgba
        .chunks_exact_mut(4)
        .zip(source_rgba.chunks_exact(4))
    {
        let alpha = (u32::from(src[3]) * u32::from(opacity_weight) + 127) / 255;
        if alpha == 0 {
            continue;
        }
        if alpha >= u32::from(u8::MAX) {
            dst[0] = src[0];
            dst[1] = src[1];
            dst[2] = src[2];
            dst[3] = u8::MAX;
            continue;
        }

        let inverse_alpha = u32::from(u8::MAX) - alpha;
        dst[0] = u8::try_from(
            ((u32::from(dst[0]) * inverse_alpha) + (u32::from(src[0]) * alpha) + 127) / 255,
        )
        .expect("alpha blend should remain within byte range");
        dst[1] = u8::try_from(
            ((u32::from(dst[1]) * inverse_alpha) + (u32::from(src[1]) * alpha) + 127) / 255,
        )
        .expect("alpha blend should remain within byte range");
        dst[2] = u8::try_from(
            ((u32::from(dst[2]) * inverse_alpha) + (u32::from(src[2]) * alpha) + 127) / 255,
        )
        .expect("alpha blend should remain within byte range");
        dst[3] = u8::MAX;
    }
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
