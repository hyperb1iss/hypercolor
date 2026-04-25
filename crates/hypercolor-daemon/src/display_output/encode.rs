//! JPEG encoding and LCD-oriented byte-space brightness scaling for display frames.

use anyhow::{Context, Result};
use fast_image_resize as fr;
use tracing::debug;
use turbojpeg::{
    Compressor as TurboJpegCompressor, Image as TurboJpegImage,
    PixelFormat as TurboJpegPixelFormat, Subsamp as TurboJpegSubsamp,
    compressed_buf_len as turbojpeg_compressed_buf_len,
};

use hypercolor_core::blend_math::{
    RgbaBlendMode, blend_rgba_pixels_in_place, decode_srgb_channel, encode_srgb_channel,
    screen_blend,
};
use hypercolor_core::bus::CanvasFrame;
use hypercolor_types::device::DisplayFrameFormat;
use hypercolor_types::scene::DisplayFaceBlendMode;

use super::render::{
    PreparedDisplayPlan, apply_circular_mask, apply_circular_mask_rgba, fast_display_crop,
    render_display_view, rgb_buffer_len, rgba_buffer_len,
};
use super::{DisplayGeometry, DisplayViewport};

const JPEG_QUALITY: u8 = 85;
const JPEG_SUBSAMP: TurboJpegSubsamp = TurboJpegSubsamp::Sub2x2;

pub(super) struct EncodedDisplayFrame {
    pub format: DisplayFrameFormat,
    pub data: Vec<u8>,
    pub preview_jpeg: Option<Vec<u8>>,
}

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
    frame_format: DisplayFrameFormat,
    include_preview_jpeg: bool,
    encode_state: &mut DisplayEncodeState,
) -> Result<EncodedDisplayFrame> {
    let brightness_factor = display_brightness_factor(brightness);
    if brightness_factor == 0 {
        prepare_black_frame(geometry, &mut encode_state.rgb_buffer);
        return finish_rgb_frame(geometry, frame_format, include_preview_jpeg, encode_state);
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
        finish_rgba_frame(geometry, frame_format, include_preview_jpeg, encode_state)
    } else {
        finish_rgb_frame(geometry, frame_format, include_preview_jpeg, encode_state)
    }
}

pub(super) fn encode_face_scene_blend(
    scene_source: Option<&CanvasFrame>,
    face_source: &CanvasFrame,
    viewport: &DisplayViewport,
    geometry: &DisplayGeometry,
    brightness: f32,
    face_blend_mode: DisplayFaceBlendMode,
    face_opacity: f32,
    frame_format: DisplayFrameFormat,
    include_preview_jpeg: bool,
    encode_state: &mut DisplayEncodeState,
) -> Result<EncodedDisplayFrame> {
    if let Some(scene_source) = scene_source {
        render_canvas_frame_rgba(scene_source, viewport, geometry, encode_state, false)?;
    } else {
        prepare_black_rgba_frame(geometry, &mut encode_state.rgba_buffer);
    }

    render_direct_canvas_frame_rgba(face_source, geometry, encode_state, true)?;
    blend_face_rgba_with_scene(
        &mut encode_state.rgba_buffer,
        &encode_state.scratch_rgba_buffer,
        face_blend_mode,
        face_opacity,
    );
    encode_prepared_rgba_frame(
        geometry,
        brightness,
        frame_format,
        include_preview_jpeg,
        encode_state,
    )
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

pub(super) fn encode_prepared_rgba_frame(
    geometry: &DisplayGeometry,
    brightness: f32,
    frame_format: DisplayFrameFormat,
    include_preview_jpeg: bool,
    encode_state: &mut DisplayEncodeState,
) -> Result<EncodedDisplayFrame> {
    let brightness_factor = display_brightness_factor(brightness);
    if brightness_factor == 0 {
        prepare_black_rgba_frame(geometry, &mut encode_state.rgba_buffer);
        return finish_rgba_frame(geometry, frame_format, include_preview_jpeg, encode_state);
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

    finish_rgba_frame(geometry, frame_format, include_preview_jpeg, encode_state)
}

fn apply_display_brightness_rgba(rgba_buffer: &mut [u8], brightness_lut: &[u8; 256]) {
    for pixel in rgba_buffer.chunks_exact_mut(4) {
        pixel[0] = brightness_lut[usize::from(pixel[0])];
        pixel[1] = brightness_lut[usize::from(pixel[1])];
        pixel[2] = brightness_lut[usize::from(pixel[2])];
    }
}

fn finish_rgb_frame(
    geometry: &DisplayGeometry,
    frame_format: DisplayFrameFormat,
    include_preview_jpeg: bool,
    encode_state: &mut DisplayEncodeState,
) -> Result<EncodedDisplayFrame> {
    match frame_format {
        DisplayFrameFormat::Jpeg => Ok(EncodedDisplayFrame {
            format: DisplayFrameFormat::Jpeg,
            data: encode_rgb_to_jpeg(geometry, encode_state)?,
            preview_jpeg: None,
        }),
        DisplayFrameFormat::Rgb => {
            let preview_jpeg = include_preview_jpeg
                .then(|| encode_rgb_to_jpeg(geometry, encode_state))
                .transpose()?;
            Ok(EncodedDisplayFrame {
                format: DisplayFrameFormat::Rgb,
                data: take_rgb_frame_data(geometry, encode_state)?,
                preview_jpeg,
            })
        }
    }
}

fn finish_rgba_frame(
    geometry: &DisplayGeometry,
    frame_format: DisplayFrameFormat,
    include_preview_jpeg: bool,
    encode_state: &mut DisplayEncodeState,
) -> Result<EncodedDisplayFrame> {
    match frame_format {
        DisplayFrameFormat::Jpeg => Ok(EncodedDisplayFrame {
            format: DisplayFrameFormat::Jpeg,
            data: encode_rgba_to_jpeg(geometry, encode_state)?,
            preview_jpeg: None,
        }),
        DisplayFrameFormat::Rgb => {
            let preview_jpeg = include_preview_jpeg
                .then(|| encode_rgba_to_jpeg(geometry, encode_state))
                .transpose()?;
            copy_rgba_to_rgb(geometry, encode_state)?;
            Ok(EncodedDisplayFrame {
                format: DisplayFrameFormat::Rgb,
                data: take_rgb_frame_data(geometry, encode_state)?,
                preview_jpeg,
            })
        }
    }
}

fn take_rgb_frame_data(
    geometry: &DisplayGeometry,
    encode_state: &mut DisplayEncodeState,
) -> Result<Vec<u8>> {
    let required_len =
        rgb_buffer_len(geometry.width, geometry.height).context("display RGB buffer overflow")?;
    let mut data = std::mem::take(&mut encode_state.rgb_buffer);
    data.truncate(required_len);
    Ok(data)
}

fn copy_rgba_to_rgb(
    geometry: &DisplayGeometry,
    encode_state: &mut DisplayEncodeState,
) -> Result<()> {
    let required_len =
        rgb_buffer_len(geometry.width, geometry.height).context("display RGB buffer overflow")?;
    encode_state.rgb_buffer.resize(required_len, 0);
    for (rgb, rgba) in encode_state
        .rgb_buffer
        .chunks_exact_mut(3)
        .zip(encode_state.rgba_buffer.chunks_exact(4))
    {
        rgb.copy_from_slice(&rgba[..3]);
    }
    Ok(())
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
    // Effect canvases are opaque (α=255). Skipping the default
    // premultiply/un-premultiply bracket roughly halves the display
    // output resize cost, which also feeds the JPEG encoder downstream.
    let options = fr::ResizeOptions::new()
        .resize_alg(fr::ResizeAlg::Interpolation(fr::FilterType::Bilinear))
        .use_alpha(false)
        .crop(crop.left, crop.top, crop.width, crop.height);
    encode_state
        .fast_resizer
        .resize(&src_image, &mut dst_image, &options)
        .context("fast display resize failed")?;

    Ok(true)
}

fn promote_rgb_to_rgba(rgb_buffer: &[u8], rgba_buffer: &mut Vec<u8>, width: u32, height: u32) {
    let Some(render_len) = rgba_buffer_len(width, height) else {
        rgba_buffer.clear();
        return;
    };
    if rgba_buffer.len() != render_len {
        rgba_buffer.resize(render_len, 0);
    }

    for (rgba, rgb) in rgba_buffer
        .chunks_exact_mut(4)
        .zip(rgb_buffer.chunks_exact(3))
    {
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

fn blend_face_rgba_with_scene(
    target_rgba: &mut [u8],
    source_rgba: &[u8],
    blend_mode: DisplayFaceBlendMode,
    opacity: f32,
) {
    match blend_mode {
        DisplayFaceBlendMode::Replace => {
            replace_face_rgba_in_place(target_rgba, source_rgba, opacity);
        }
        DisplayFaceBlendMode::Tint => {
            blend_face_material_tint_rgba(target_rgba, source_rgba, opacity);
        }
        DisplayFaceBlendMode::LumaReveal => {
            blend_face_luma_reveal_rgba(target_rgba, source_rgba, opacity);
        }
        _ => {
            let Some(canvas_blend_mode) = blend_mode.standard_canvas_blend_mode() else {
                return;
            };
            blend_rgba_pixels_in_place(
                target_rgba,
                source_rgba,
                RgbaBlendMode::from(canvas_blend_mode),
                opacity,
            );
        }
    }

    for pixel in target_rgba.chunks_exact_mut(4) {
        pixel[3] = u8::MAX;
    }
}

fn replace_face_rgba_in_place(target_rgba: &mut [u8], source_rgba: &[u8], opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    for (target_pixel, source_pixel) in target_rgba
        .chunks_exact_mut(4)
        .zip(source_rgba.chunks_exact(4))
    {
        let source_alpha = (f32::from(source_pixel[3]) / 255.0) * opacity;
        target_pixel[0] = encode_srgb_channel(decode_srgb_channel(source_pixel[0]) * source_alpha);
        target_pixel[1] = encode_srgb_channel(decode_srgb_channel(source_pixel[1]) * source_alpha);
        target_pixel[2] = encode_srgb_channel(decode_srgb_channel(source_pixel[2]) * source_alpha);
        target_pixel[3] = u8::MAX;
    }
}

fn blend_face_material_tint_rgba(target_rgba: &mut [u8], source_rgba: &[u8], opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }

    for (dst_px, src_px) in target_rgba
        .chunks_exact_mut(4)
        .zip(source_rgba.chunks_exact(4))
    {
        let alpha = (f32::from(src_px[3]) / 255.0) * opacity;
        if alpha <= 0.0 {
            continue;
        }

        let dst = [
            decode_srgb_channel(dst_px[0]),
            decode_srgb_channel(dst_px[1]),
            decode_srgb_channel(dst_px[2]),
        ];
        let src = [
            decode_srgb_channel(src_px[0]),
            decode_srgb_channel(src_px[1]),
            decode_srgb_channel(src_px[2]),
        ];
        let material = effect_tint_material(dst, src);

        dst_px[0] = encode_srgb_channel(dst[0].mul_add(1.0 - alpha, material[0] * alpha));
        dst_px[1] = encode_srgb_channel(dst[1].mul_add(1.0 - alpha, material[1] * alpha));
        dst_px[2] = encode_srgb_channel(dst[2].mul_add(1.0 - alpha, material[2] * alpha));
    }
}

fn blend_face_luma_reveal_rgba(target_rgba: &mut [u8], source_rgba: &[u8], opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }

    for (dst_px, src_px) in target_rgba
        .chunks_exact_mut(4)
        .zip(source_rgba.chunks_exact(4))
    {
        let alpha = (f32::from(src_px[3]) / 255.0) * opacity;
        if alpha <= 0.0 {
            continue;
        }

        let dst = [
            decode_srgb_channel(dst_px[0]),
            decode_srgb_channel(dst_px[1]),
            decode_srgb_channel(dst_px[2]),
        ];
        let src = [
            decode_srgb_channel(src_px[0]),
            decode_srgb_channel(src_px[1]),
            decode_srgb_channel(src_px[2]),
        ];
        let material = effect_tint_material(dst, src);
        let reveal = smoothstep(0.18, 0.92, linear_rgb_luma(src));
        let inside = [
            src[0].mul_add(1.0 - reveal, material[0] * reveal),
            src[1].mul_add(1.0 - reveal, material[1] * reveal),
            src[2].mul_add(1.0 - reveal, material[2] * reveal),
        ];

        dst_px[0] = encode_srgb_channel(dst[0].mul_add(1.0 - alpha, inside[0] * alpha));
        dst_px[1] = encode_srgb_channel(dst[1].mul_add(1.0 - alpha, inside[1] * alpha));
        dst_px[2] = encode_srgb_channel(dst[2].mul_add(1.0 - alpha, inside[2] * alpha));
    }
}

fn effect_tint_material(effect_rgb: [f32; 3], face_rgb: [f32; 3]) -> [f32; 3] {
    let luma = linear_rgb_luma(face_rgb);
    let colorfulness = rgb_colorfulness(face_rgb);
    let neutral = 0.18_f32.mul_add(1.0 - luma, luma).clamp(0.18, 1.0);
    let emission_strength = (1.0 - colorfulness) * luma * 0.12;

    std::array::from_fn(|index| {
        let tint = neutral.mul_add(1.0 - 0.72, face_rgb[index].max(neutral * 0.75) * 0.72);
        let filtered = effect_rgb[index] * tint;
        screen_blend(filtered, face_rgb[index] * emission_strength)
    })
}

fn linear_rgb_luma(rgb: [f32; 3]) -> f32 {
    (rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722).clamp(0.0, 1.0)
}

fn rgb_colorfulness(rgb: [f32; 3]) -> f32 {
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    (max - min).clamp(0.0, 1.0)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge0 >= edge1 {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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

    if rgb_buffer.len() == render_len {
        rgb_buffer.fill(0);
    } else {
        rgb_buffer.resize(render_len, 0);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_brightness_factor_clamps_and_rounds_to_byte_policy() {
        assert_eq!(display_brightness_factor(-1.0), 0);
        assert_eq!(display_brightness_factor(f32::NAN), 0);
        assert_eq!(display_brightness_factor(0.0), 0);
        assert_eq!(display_brightness_factor(0.5), 128);
        assert_eq!(display_brightness_factor(1.0), 255);
        assert_eq!(display_brightness_factor(2.0), 255);
    }

    #[test]
    fn display_brightness_scales_srgb_bytes_not_linear_light() {
        let half_brightness = display_brightness_factor(0.5);

        assert_eq!(scale_channel(0, half_brightness), 0);
        assert_eq!(scale_channel(128, half_brightness), 64);
        assert_eq!(scale_channel(255, half_brightness), 128);
    }

    #[test]
    fn display_brightness_lut_reuses_policy_for_rgba_frames() {
        let mut state = DisplayEncodeState::new().expect("display encoder should initialize");
        refresh_display_brightness_lut(&mut state, display_brightness_factor(0.5));
        let mut rgba = [255, 128, 64, 127, 10, 20, 30, 255];

        apply_display_brightness_rgba(&mut rgba, &state.brightness_lut);

        assert_eq!(rgba, [128, 64, 32, 127, 5, 10, 15, 255]);
    }
}
