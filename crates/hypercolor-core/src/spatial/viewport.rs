use hypercolor_types::canvas::{Canvas, Rgba, RgbaF32};
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

#[allow(clippy::cast_precision_loss, clippy::as_conversions)]
fn blit_stretch(canvas: &mut Canvas, source: &Canvas, crop: PixelRect, brightness: f32) {
    let out_width = canvas.width().max(1) as f32;
    let out_height = canvas.height().max(1) as f32;
    for y in 0..canvas.height() {
        let ny = (y as f32 + 0.5) / out_height;
        for x in 0..canvas.width() {
            let nx = (x as f32 + 0.5) / out_width;
            let pixel = sample_source(
                source,
                crop.x as f32 + nx * crop.width.max(1) as f32,
                crop.y as f32 + ny * crop.height.max(1) as f32,
                brightness,
            );
            canvas.set_pixel(x, y, pixel);
        }
    }
}

#[allow(clippy::cast_precision_loss, clippy::as_conversions)]
fn blit_contain(canvas: &mut Canvas, source: &Canvas, crop: PixelRect, brightness: f32) {
    let crop_aspect = crop.width.max(1) as f32 / crop.height.max(1) as f32;
    let out_width = canvas.width().max(1) as f32;
    let out_height = canvas.height().max(1) as f32;
    let out_aspect = out_width / out_height;

    let (draw_width, draw_height) = if out_aspect > crop_aspect {
        (out_height * crop_aspect, out_height)
    } else {
        (out_width, out_width / crop_aspect)
    };
    let offset_x = (out_width - draw_width) * 0.5;
    let offset_y = (out_height - draw_height) * 0.5;

    for y in 0..canvas.height() {
        let yf = y as f32 + 0.5;
        if yf < offset_y || yf > offset_y + draw_height {
            continue;
        }
        let ny = ((yf - offset_y) / draw_height).clamp(0.0, 1.0);
        for x in 0..canvas.width() {
            let xf = x as f32 + 0.5;
            if xf < offset_x || xf > offset_x + draw_width {
                continue;
            }
            let nx = ((xf - offset_x) / draw_width).clamp(0.0, 1.0);
            let pixel = sample_source(
                source,
                crop.x as f32 + nx * crop.width.max(1) as f32,
                crop.y as f32 + ny * crop.height.max(1) as f32,
                brightness,
            );
            canvas.set_pixel(x, y, pixel);
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
        fitted.y = fitted.y.saturating_add(crop.height.saturating_sub(fitted.height) / 2);
    } else if out_aspect < crop_aspect {
        fitted.width = ((crop.height.max(1) as f32) * out_aspect).max(1.0).round() as u32;
        fitted.x = fitted.x.saturating_add(crop.width.saturating_sub(fitted.width) / 2);
    }

    blit_stretch(canvas, source, fitted, brightness);
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn sample_source(source: &Canvas, x: f32, y: f32, brightness: f32) -> Rgba {
    let src_x = x
        .floor()
        .clamp(0.0, source.width().saturating_sub(1) as f32) as u32;
    let src_y = y
        .floor()
        .clamp(0.0, source.height().saturating_sub(1) as f32) as u32;
    let pixel = source.get_pixel(src_x, src_y).to_linear_f32();
    let scaled = RgbaF32::new(
        pixel.r * brightness,
        pixel.g * brightness,
        pixel.b * brightness,
        pixel.a,
    );
    scaled.to_srgba()
}
