use hypercolor_types::canvas::Rgba;

#[derive(Clone, Copy)]
pub(super) enum PreviewScaleFormat {
    Rgb,
    Rgba,
}

impl PreviewScaleFormat {
    const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
pub(super) fn scale_rgba_bilinear(
    rgba: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    brightness_lut: Option<&[u8; 256]>,
    format: PreviewScaleFormat,
    out: &mut Vec<u8>,
) {
    let source_width = usize::try_from(source_width).unwrap_or(0);
    let source_height = usize::try_from(source_height).unwrap_or(0);
    let target_width = usize::try_from(target_width).unwrap_or(0);
    let target_height = usize::try_from(target_height).unwrap_or(0);
    let out_bpp = format.bytes_per_pixel();
    let required_len = target_width
        .saturating_mul(target_height)
        .saturating_mul(out_bpp);
    if out.len() != required_len {
        out.resize(required_len, 0);
    }

    if source_width == 0
        || source_height == 0
        || target_width == 0
        || target_height == 0
        || rgba.len() < source_width.saturating_mul(source_height).saturating_mul(4)
    {
        out.fill(0);
        return;
    }

    let max_x = source_width.saturating_sub(1);
    let max_y = source_height.saturating_sub(1);

    for y in 0..target_height {
        let source_y = sample_axis(y, target_height, source_height);
        let y0 = source_y.floor().clamp(0.0, max_y as f32) as usize;
        let y1 = y0.saturating_add(1).min(max_y);
        let ty = source_y - y0 as f32;

        for x in 0..target_width {
            let source_x = sample_axis(x, target_width, source_width);
            let x0 = source_x.floor().clamp(0.0, max_x as f32) as usize;
            let x1 = x0.saturating_add(1).min(max_x);
            let tx = source_x - x0 as f32;

            let p00 = sample_pixel(rgba, source_width, x0, y0);
            let p10 = sample_pixel(rgba, source_width, x1, y0);
            let p01 = sample_pixel(rgba, source_width, x0, y1);
            let p11 = sample_pixel(rgba, source_width, x1, y1);

            let pixel = bilerp_pixel(&p00, &p10, &p01, &p11, tx, ty);
            let out_offset = y
                .saturating_mul(target_width)
                .saturating_add(x)
                .saturating_mul(out_bpp);

            out[out_offset] = apply_brightness(pixel.r, brightness_lut);
            out[out_offset + 1] = apply_brightness(pixel.g, brightness_lut);
            out[out_offset + 2] = apply_brightness(pixel.b, brightness_lut);
            if matches!(format, PreviewScaleFormat::Rgba) {
                out[out_offset + 3] = pixel.a;
            }
        }
    }
}

#[allow(clippy::cast_precision_loss, clippy::as_conversions)]
fn sample_axis(target_index: usize, target_extent: usize, source_extent: usize) -> f32 {
    ((target_index as f32 + 0.5) * source_extent as f32 / target_extent.max(1) as f32 - 0.5)
        .clamp(0.0, source_extent.saturating_sub(1) as f32)
}

fn sample_pixel(rgba: &[u8], width: usize, x: usize, y: usize) -> Rgba {
    let offset = y.saturating_mul(width).saturating_add(x).saturating_mul(4);
    Rgba::new(
        rgba[offset],
        rgba[offset + 1],
        rgba[offset + 2],
        rgba[offset + 3],
    )
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn bilerp_pixel(p00: &Rgba, p10: &Rgba, p01: &Rgba, p11: &Rgba, tx: f32, ty: f32) -> Rgba {
    let top_r = lerp_channel(p00.r, p10.r, tx);
    let top_g = lerp_channel(p00.g, p10.g, tx);
    let top_b = lerp_channel(p00.b, p10.b, tx);
    let top_a = lerp_channel(p00.a, p10.a, tx);
    let bottom_r = lerp_channel(p01.r, p11.r, tx);
    let bottom_g = lerp_channel(p01.g, p11.g, tx);
    let bottom_b = lerp_channel(p01.b, p11.b, tx);
    let bottom_a = lerp_channel(p01.a, p11.a, tx);

    Rgba::new(
        lerp_scalar(top_r, bottom_r, ty).round().clamp(0.0, 255.0) as u8,
        lerp_scalar(top_g, bottom_g, ty).round().clamp(0.0, 255.0) as u8,
        lerp_scalar(top_b, bottom_b, ty).round().clamp(0.0, 255.0) as u8,
        lerp_scalar(top_a, bottom_a, ty).round().clamp(0.0, 255.0) as u8,
    )
}

fn lerp_channel(left: u8, right: u8, t: f32) -> f32 {
    lerp_scalar(f32::from(left), f32::from(right), t)
}

fn lerp_scalar(left: f32, right: f32, t: f32) -> f32 {
    left + (right - left) * t
}

fn apply_brightness(channel: u8, brightness_lut: Option<&[u8; 256]>) -> u8 {
    brightness_lut.map_or(channel, |lut| lut[usize::from(channel)])
}

#[cfg(test)]
mod tests {
    use super::{PreviewScaleFormat, scale_rgba_bilinear};

    #[test]
    fn bilinear_scaling_preserves_identity_at_native_resolution() {
        let source = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let mut out = Vec::new();
        scale_rgba_bilinear(
            &source,
            2,
            2,
            2,
            2,
            None,
            PreviewScaleFormat::Rgba,
            &mut out,
        );
        assert_eq!(out, source);
    }

    #[test]
    fn bilinear_scaling_averages_center_pixels() {
        let source = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let mut out = Vec::new();
        scale_rgba_bilinear(
            &source,
            2,
            2,
            1,
            1,
            None,
            PreviewScaleFormat::Rgba,
            &mut out,
        );
        assert_eq!(out, vec![128, 128, 128, 255]);
    }

    #[test]
    fn bilinear_scaling_applies_brightness_after_interpolation() {
        let source = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let brightness_lut = std::array::from_fn(|channel| {
            u8::try_from(channel / 2).expect("brightness LUT should stay in byte range")
        });
        let mut out = Vec::new();
        scale_rgba_bilinear(
            &source,
            2,
            2,
            1,
            1,
            Some(&brightness_lut),
            PreviewScaleFormat::Rgb,
            &mut out,
        );
        assert_eq!(out, vec![64, 64, 64]);
    }
}
