use std::cell::RefCell;

use fast_image_resize as fr;
use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas, srgb_u8_to_linear};
use hypercolor_types::canvas::{linear_to_srgb_u8};
use hypercolor_types::viewport::{FitMode, PixelRect, ViewportRect};

/// Per-thread resizer + intermediate scratch for the `Contain` path. The
/// resizer's internal scratch is rebuilt lazily the first time each
/// resize ratio is requested, so keeping it alive across calls means
/// every tokio worker pays the setup cost at most once per ratio.
struct ViewportState {
    resizer: fr::Resizer,
    /// Holds the resized-but-not-yet-blitted crop when Contain letterboxes
    /// the output. Stretch and Cover write straight into the target and
    /// leave this buffer untouched.
    contain_scratch: Vec<u8>,
}

impl ViewportState {
    fn new() -> Self {
        Self {
            resizer: fr::Resizer::new(),
            contain_scratch: Vec::new(),
        }
    }
}

thread_local! {
    static VIEWPORT_STATE: RefCell<ViewportState> = RefCell::new(ViewportState::new());
}

pub fn sample_viewport(
    target: &mut Canvas,
    source: &Canvas,
    viewport: ViewportRect,
    fit_mode: FitMode,
    brightness: f32,
) {
    let crop = viewport.to_pixel_rect(source.width(), source.height());
    match fit_mode {
        FitMode::Stretch => blit_stretch(target, source, crop, brightness),
        FitMode::Contain => blit_contain(target, source, crop, brightness),
        FitMode::Cover => blit_cover(target, source, crop, brightness),
    }
}

#[allow(clippy::cast_lossless, clippy::as_conversions)]
fn blit_stretch(target: &mut Canvas, source: &Canvas, crop: PixelRect, brightness: f32) {
    let source_width = source.width();
    let source_height = source.height();
    let target_width = target.width();
    let target_height = target.height();
    if source_width == 0
        || source_height == 0
        || target_width == 0
        || target_height == 0
        || crop.width == 0
        || crop.height == 0
    {
        return;
    }

    let expected_source_len = (source_width as usize)
        .saturating_mul(source_height as usize)
        .saturating_mul(BYTES_PER_PIXEL);
    if source.as_rgba_bytes().len() < expected_source_len {
        return;
    }

    VIEWPORT_STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        let resizer = &mut state.resizer;

        let Ok(src_image) = fr::images::ImageRef::new(
            source_width,
            source_height,
            source.as_rgba_bytes(),
            fr::PixelType::U8x4,
        ) else {
            return;
        };

        {
            let target_bytes = target.as_rgba_bytes_mut();
            let Ok(mut dst_image) = fr::images::Image::from_slice_u8(
                target_width,
                target_height,
                target_bytes,
                fr::PixelType::U8x4,
            ) else {
                return;
            };
            let options = fr::ResizeOptions::new()
                .resize_alg(fr::ResizeAlg::Interpolation(fr::FilterType::Bilinear))
                .crop(
                    f64::from(crop.x),
                    f64::from(crop.y),
                    f64::from(crop.width),
                    f64::from(crop.height),
                );
            if resizer.resize(&src_image, &mut dst_image, &options).is_err() {
                return;
            }
        }

        if brightness_needs_lut(brightness) {
            let lut = build_brightness_lut(brightness);
            apply_brightness_lut_rgb(target.as_rgba_bytes_mut(), &lut);
        }
    });
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    clippy::cast_lossless
)]
fn blit_contain(target: &mut Canvas, source: &Canvas, crop: PixelRect, brightness: f32) {
    let source_width = source.width();
    let source_height = source.height();
    let target_width = target.width();
    let target_height = target.height();
    if source_width == 0
        || source_height == 0
        || target_width == 0
        || target_height == 0
        || crop.width == 0
        || crop.height == 0
    {
        return;
    }

    let expected_source_len = (source_width as usize)
        .saturating_mul(source_height as usize)
        .saturating_mul(BYTES_PER_PIXEL);
    if source.as_rgba_bytes().len() < expected_source_len {
        return;
    }

    let crop_aspect = (crop.width.max(1) as f32) / (crop.height.max(1) as f32);
    let out_width_f = target_width.max(1) as f32;
    let out_height_f = target_height.max(1) as f32;
    let out_aspect = out_width_f / out_height_f;

    let (draw_width_f, draw_height_f) = if out_aspect > crop_aspect {
        (out_height_f * crop_aspect, out_height_f)
    } else {
        (out_width_f, out_width_f / crop_aspect)
    };
    // Snap the drawable rect to integer pixels so the letterboxed region
    // aligns with the canvas grid (preserving the existing contract that
    // rows outside the draw rect are left untouched).
    let draw_w_raw = draw_width_f.round().max(1.0) as u32;
    let draw_h_raw = draw_height_f.round().max(1.0) as u32;
    let offset_x = ((out_width_f - draw_width_f) * 0.5).round().max(0.0) as u32;
    let offset_y = ((out_height_f - draw_height_f) * 0.5).round().max(0.0) as u32;
    let draw_w = draw_w_raw.min(target_width.saturating_sub(offset_x));
    let draw_h = draw_h_raw.min(target_height.saturating_sub(offset_y));
    if draw_w == 0 || draw_h == 0 {
        return;
    }

    let draw_w_usize = draw_w as usize;
    let draw_h_usize = draw_h as usize;
    let intermediate_len = draw_w_usize
        .saturating_mul(draw_h_usize)
        .saturating_mul(BYTES_PER_PIXEL);

    VIEWPORT_STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        let ViewportState {
            resizer,
            contain_scratch,
        } = &mut *state;

        if contain_scratch.len() != intermediate_len {
            contain_scratch.resize(intermediate_len, 0);
        }

        let Ok(src_image) = fr::images::ImageRef::new(
            source_width,
            source_height,
            source.as_rgba_bytes(),
            fr::PixelType::U8x4,
        ) else {
            return;
        };
        {
            let Ok(mut dst_image) = fr::images::Image::from_slice_u8(
                draw_w,
                draw_h,
                contain_scratch.as_mut_slice(),
                fr::PixelType::U8x4,
            ) else {
                return;
            };
            let options = fr::ResizeOptions::new()
                .resize_alg(fr::ResizeAlg::Interpolation(fr::FilterType::Bilinear))
                .crop(
                    f64::from(crop.x),
                    f64::from(crop.y),
                    f64::from(crop.width),
                    f64::from(crop.height),
                );
            if resizer.resize(&src_image, &mut dst_image, &options).is_err() {
                return;
            }
        }

        // Blit the intermediate into the target at the draw rect. Rows
        // outside the draw rect are left as-is — the existing contract
        // is "callers clear first if they want a clean letterbox," and
        // production callers (`web_viewport.rs`, etc.) do exactly that.
        let target_bytes = target.as_rgba_bytes_mut();
        let target_stride = (target_width as usize) * BYTES_PER_PIXEL;
        let intermediate_stride = draw_w_usize * BYTES_PER_PIXEL;
        let offset_x_bytes = (offset_x as usize) * BYTES_PER_PIXEL;
        for row in 0..draw_h_usize {
            let dst_start = ((offset_y as usize) + row) * target_stride + offset_x_bytes;
            let dst_end = dst_start + intermediate_stride;
            let src_start = row * intermediate_stride;
            let src_end = src_start + intermediate_stride;
            target_bytes[dst_start..dst_end]
                .copy_from_slice(&contain_scratch[src_start..src_end]);
        }

        if brightness_needs_lut(brightness) {
            let lut = build_brightness_lut(brightness);
            for row in 0..draw_h_usize {
                let dst_start = ((offset_y as usize) + row) * target_stride + offset_x_bytes;
                let dst_end = dst_start + intermediate_stride;
                apply_brightness_lut_rgb(&mut target_bytes[dst_start..dst_end], &lut);
            }
        }
    });
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn blit_cover(target: &mut Canvas, source: &Canvas, crop: PixelRect, brightness: f32) {
    let out_aspect = (target.width().max(1) as f32) / (target.height().max(1) as f32);
    let crop_aspect = (crop.width.max(1) as f32) / (crop.height.max(1) as f32);
    let mut fitted = crop;

    if out_aspect > crop_aspect {
        fitted.height = ((crop.width.max(1) as f32) / out_aspect).max(1.0).round() as u32;
        fitted.y = fitted
            .y
            .saturating_add(crop.height.saturating_sub(fitted.height) / 2);
    } else if out_aspect < crop_aspect {
        fitted.width = ((crop.height.max(1) as f32) * out_aspect).max(1.0).round() as u32;
        fitted.x = fitted
            .x
            .saturating_add(crop.width.saturating_sub(fitted.width) / 2);
    }

    blit_stretch(target, source, fitted, brightness);
}

/// True when `brightness` meaningfully differs from 1.0 (outside one LSB
/// of typical rounding noise). Avoids spending 256 LUT builds when the
/// control slider is sitting at full brightness.
fn brightness_needs_lut(brightness: f32) -> bool {
    (brightness - 1.0).abs() > 1.0e-3
}

/// Build a 256-entry LUT that applies `brightness` in linear light and
/// re-encodes to sRGB bytes. This keeps the "brightness slider is
/// gamma-correct" semantics from the old scalar blit even though the
/// bilinear resize itself now runs in sRGB space for speed.
fn build_brightness_lut(brightness: f32) -> [u8; 256] {
    let mut lut = [0_u8; 256];
    for (index, entry) in lut.iter_mut().enumerate() {
        // `srgb_u8_to_linear` is already an inlined table read; the
        // multiply happens in linear light so black→black regardless of
        // the brightness value, and saturation at 1.0 is handled by
        // `linear_to_srgb_u8`'s internal clamp.
        #[allow(clippy::cast_possible_truncation)]
        let srgb_byte = index as u8;
        let linear = srgb_u8_to_linear(srgb_byte) * brightness;
        *entry = linear_to_srgb_u8(linear);
    }
    lut
}

/// Apply a precomputed brightness LUT to the R/G/B bytes of an RGBA
/// slice, leaving alpha untouched. Auto-vectorizes cleanly on AVX2.
fn apply_brightness_lut_rgb(bytes: &mut [u8], lut: &[u8; 256]) {
    for chunk in bytes.chunks_exact_mut(BYTES_PER_PIXEL) {
        chunk[0] = lut[usize::from(chunk[0])];
        chunk[1] = lut[usize::from(chunk[1])];
        chunk[2] = lut[usize::from(chunk[2])];
    }
}
