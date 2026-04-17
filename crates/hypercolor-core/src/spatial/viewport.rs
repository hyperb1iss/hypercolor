use hypercolor_types::canvas::{
    BYTES_PER_PIXEL, Canvas, linear_to_srgb_u8, srgb_u8_to_linear,
};
use hypercolor_types::viewport::{FitMode, PixelRect, ViewportRect};

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

/// Raw-slice bilinear blit with gamma-correct interpolation.
///
/// Lifts `Arc::make_mut` out of the pixel loop (one call per frame instead
/// of one per pixel), skips bounds-checked `get_pixel`/`set_pixel`, and
/// reads sRGB bytes straight through the precomputed LUT. At 1280x1024
/// this is the difference between ~60 ms and ~10 ms per viewport blit.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn blit_stretch(target: &mut Canvas, source: &Canvas, crop: PixelRect, brightness: f32) {
    let target_width = target.width();
    let target_height = target.height();
    let source_width = source.width();
    let source_height = source.height();
    if target_width == 0 || target_height == 0 || source_width == 0 || source_height == 0 {
        return;
    }

    let source_bytes = source.as_rgba_bytes();
    let expected_source_len = (source_width as usize)
        .saturating_mul(source_height as usize)
        .saturating_mul(BYTES_PER_PIXEL);
    if source_bytes.len() < expected_source_len {
        return;
    }

    let target_bytes = target.as_rgba_bytes_mut();
    let expected_target_len = (target_width as usize)
        .saturating_mul(target_height as usize)
        .saturating_mul(BYTES_PER_PIXEL);
    if target_bytes.len() < expected_target_len {
        return;
    }

    let source_max_x = source_width.saturating_sub(1);
    let source_max_y = source_height.saturating_sub(1);
    let source_max_x_f = source_max_x as f32;
    let source_max_y_f = source_max_y as f32;
    let source_stride = (source_width as usize).saturating_mul(BYTES_PER_PIXEL);
    let target_stride = (target_width as usize).saturating_mul(BYTES_PER_PIXEL);

    let crop_x = crop.x as f32;
    let crop_y = crop.y as f32;
    let x_span = crop.width.saturating_sub(1) as f32;
    let y_span = crop.height.saturating_sub(1) as f32;
    let tx_divisor = (target_width.saturating_sub(1)).max(1) as f32;
    let ty_divisor = (target_height.saturating_sub(1)).max(1) as f32;

    for out_y in 0..target_height {
        let source_y = if target_height <= 1 {
            crop_y + y_span * 0.5
        } else {
            crop_y + (out_y as f32 * y_span) / ty_divisor
        };
        let clamped_y = source_y.clamp(0.0, source_max_y_f);
        let y0 = clamped_y.floor() as u32;
        let y1 = y0.saturating_add(1).min(source_max_y);
        let fy = clamped_y - (y0 as f32);
        let fy_inv = 1.0 - fy;

        let row0_base = (y0 as usize) * source_stride;
        let row1_base = (y1 as usize) * source_stride;
        let out_row_base = (out_y as usize) * target_stride;

        for out_x in 0..target_width {
            let source_x = if target_width <= 1 {
                crop_x + x_span * 0.5
            } else {
                crop_x + (out_x as f32 * x_span) / tx_divisor
            };
            let clamped_x = source_x.clamp(0.0, source_max_x_f);
            let x0 = clamped_x.floor() as u32;
            let x1 = x0.saturating_add(1).min(source_max_x);
            let fx = clamped_x - (x0 as f32);
            let fx_inv = 1.0 - fx;

            let off00 = row0_base + (x0 as usize) * BYTES_PER_PIXEL;
            let off10 = row0_base + (x1 as usize) * BYTES_PER_PIXEL;
            let off01 = row1_base + (x0 as usize) * BYTES_PER_PIXEL;
            let off11 = row1_base + (x1 as usize) * BYTES_PER_PIXEL;

            // Bilinear weight for each tap — computing as inv/non-inv
            // products keeps the inner math to three multiplies and one
            // add per channel, and LLVM readily FMA's it on x86_64.
            let w00 = fx_inv * fy_inv;
            let w10 = fx * fy_inv;
            let w01 = fx_inv * fy;
            let w11 = fx * fy;

            let r = w00 * srgb_u8_to_linear(source_bytes[off00])
                + w10 * srgb_u8_to_linear(source_bytes[off10])
                + w01 * srgb_u8_to_linear(source_bytes[off01])
                + w11 * srgb_u8_to_linear(source_bytes[off11]);
            let g = w00 * srgb_u8_to_linear(source_bytes[off00 + 1])
                + w10 * srgb_u8_to_linear(source_bytes[off10 + 1])
                + w01 * srgb_u8_to_linear(source_bytes[off01 + 1])
                + w11 * srgb_u8_to_linear(source_bytes[off11 + 1]);
            let b = w00 * srgb_u8_to_linear(source_bytes[off00 + 2])
                + w10 * srgb_u8_to_linear(source_bytes[off10 + 2])
                + w01 * srgb_u8_to_linear(source_bytes[off01 + 2])
                + w11 * srgb_u8_to_linear(source_bytes[off11 + 2]);
            let a = (w00 * f32::from(source_bytes[off00 + 3])
                + w10 * f32::from(source_bytes[off10 + 3])
                + w01 * f32::from(source_bytes[off01 + 3])
                + w11 * f32::from(source_bytes[off11 + 3]))
                / 255.0;

            let out_off = out_row_base + (out_x as usize) * BYTES_PER_PIXEL;
            target_bytes[out_off] = linear_to_srgb_u8(r * brightness);
            target_bytes[out_off + 1] = linear_to_srgb_u8(g * brightness);
            target_bytes[out_off + 2] = linear_to_srgb_u8(b * brightness);
            target_bytes[out_off + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

#[allow(clippy::cast_precision_loss, clippy::as_conversions)]
fn blit_contain(canvas: &mut Canvas, source: &Canvas, crop: PixelRect, brightness: f32) {
    let canvas_width = canvas.width();
    let canvas_height = canvas.height();
    let source_width = source.width();
    let source_height = source.height();
    if canvas_width == 0 || canvas_height == 0 || source_width == 0 || source_height == 0 {
        return;
    }

    let crop_aspect = crop.width.max(1) as f32 / crop.height.max(1) as f32;
    let out_width = canvas_width.max(1) as f32;
    let out_height = canvas_height.max(1) as f32;
    let out_aspect = out_width / out_height;

    let (draw_width, draw_height) = if out_aspect > crop_aspect {
        (out_height * crop_aspect, out_height)
    } else {
        (out_width, out_width / crop_aspect)
    };
    let offset_x = (out_width - draw_width) * 0.5;
    let offset_y = (out_height - draw_height) * 0.5;

    let source_bytes = source.as_rgba_bytes();
    let expected_source_len = (source_width as usize)
        .saturating_mul(source_height as usize)
        .saturating_mul(BYTES_PER_PIXEL);
    if source_bytes.len() < expected_source_len {
        return;
    }

    let target_bytes = canvas.as_rgba_bytes_mut();
    let expected_target_len = (canvas_width as usize)
        .saturating_mul(canvas_height as usize)
        .saturating_mul(BYTES_PER_PIXEL);
    if target_bytes.len() < expected_target_len {
        return;
    }

    let source_max_x = source_width.saturating_sub(1);
    let source_max_y = source_height.saturating_sub(1);
    let source_max_x_f = source_max_x as f32;
    let source_max_y_f = source_max_y as f32;
    let source_stride = (source_width as usize).saturating_mul(BYTES_PER_PIXEL);
    let target_stride = (canvas_width as usize).saturating_mul(BYTES_PER_PIXEL);

    let crop_x_f = crop.x as f32;
    let crop_y_f = crop.y as f32;
    let crop_w_f = crop.width.max(1) as f32;
    let crop_h_f = crop.height.max(1) as f32;

    for out_y in 0..canvas_height {
        let yf = out_y as f32 + 0.5;
        if yf < offset_y || yf > offset_y + draw_height {
            continue;
        }
        let ny = ((yf - offset_y) / draw_height).clamp(0.0, 1.0);
        let source_y = (crop_y_f + ny * crop_h_f - 0.5).clamp(0.0, source_max_y_f);
        let y0 = source_y.floor() as u32;
        let y1 = y0.saturating_add(1).min(source_max_y);
        let fy = source_y - (y0 as f32);
        let fy_inv = 1.0 - fy;

        let row0_base = (y0 as usize) * source_stride;
        let row1_base = (y1 as usize) * source_stride;
        let out_row_base = (out_y as usize) * target_stride;

        for out_x in 0..canvas_width {
            let xf = out_x as f32 + 0.5;
            if xf < offset_x || xf > offset_x + draw_width {
                continue;
            }
            let nx = ((xf - offset_x) / draw_width).clamp(0.0, 1.0);
            let source_x = (crop_x_f + nx * crop_w_f - 0.5).clamp(0.0, source_max_x_f);
            let x0 = source_x.floor() as u32;
            let x1 = x0.saturating_add(1).min(source_max_x);
            let fx = source_x - (x0 as f32);
            let fx_inv = 1.0 - fx;

            let off00 = row0_base + (x0 as usize) * BYTES_PER_PIXEL;
            let off10 = row0_base + (x1 as usize) * BYTES_PER_PIXEL;
            let off01 = row1_base + (x0 as usize) * BYTES_PER_PIXEL;
            let off11 = row1_base + (x1 as usize) * BYTES_PER_PIXEL;

            let w00 = fx_inv * fy_inv;
            let w10 = fx * fy_inv;
            let w01 = fx_inv * fy;
            let w11 = fx * fy;

            let r = w00 * srgb_u8_to_linear(source_bytes[off00])
                + w10 * srgb_u8_to_linear(source_bytes[off10])
                + w01 * srgb_u8_to_linear(source_bytes[off01])
                + w11 * srgb_u8_to_linear(source_bytes[off11]);
            let g = w00 * srgb_u8_to_linear(source_bytes[off00 + 1])
                + w10 * srgb_u8_to_linear(source_bytes[off10 + 1])
                + w01 * srgb_u8_to_linear(source_bytes[off01 + 1])
                + w11 * srgb_u8_to_linear(source_bytes[off11 + 1]);
            let b = w00 * srgb_u8_to_linear(source_bytes[off00 + 2])
                + w10 * srgb_u8_to_linear(source_bytes[off10 + 2])
                + w01 * srgb_u8_to_linear(source_bytes[off01 + 2])
                + w11 * srgb_u8_to_linear(source_bytes[off11 + 2]);
            let a = (w00 * f32::from(source_bytes[off00 + 3])
                + w10 * f32::from(source_bytes[off10 + 3])
                + w01 * f32::from(source_bytes[off01 + 3])
                + w11 * f32::from(source_bytes[off11 + 3]))
                / 255.0;

            let out_off = out_row_base + (out_x as usize) * BYTES_PER_PIXEL;
            target_bytes[out_off] = linear_to_srgb_u8(r * brightness);
            target_bytes[out_off + 1] = linear_to_srgb_u8(g * brightness);
            target_bytes[out_off + 2] = linear_to_srgb_u8(b * brightness);
            target_bytes[out_off + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

#[allow(clippy::cast_precision_loss, clippy::as_conversions)]
fn blit_cover(canvas: &mut Canvas, source: &Canvas, crop: PixelRect, brightness: f32) {
    let out_aspect = canvas.width().max(1) as f32 / canvas.height().max(1) as f32;
    let crop_aspect = crop.width.max(1) as f32 / crop.height.max(1) as f32;
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

    blit_stretch(canvas, source, fitted, brightness);
}
