use std::array;
use std::sync::LazyLock;

use crate::types::canvas::{BlendMode, linear_to_srgb_u8, srgb_u8_to_linear};
use crate::types::layer::LayerAdjust;

const LINEAR_ENCODE_LUT_SCALE: f32 = 65_535.0;
const CHANNEL_PAIR_LUT_SIZE: usize = 256 * 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RgbaBlendMode {
    Normal,
    Add,
    Screen,
    Multiply,
    Overlay,
    SoftLight,
    ColorDodge,
    Difference,
}

impl From<BlendMode> for RgbaBlendMode {
    fn from(value: BlendMode) -> Self {
        match value {
            BlendMode::Normal => Self::Normal,
            BlendMode::Add => Self::Add,
            BlendMode::Screen => Self::Screen,
            BlendMode::Multiply => Self::Multiply,
            BlendMode::Overlay => Self::Overlay,
            BlendMode::SoftLight => Self::SoftLight,
            BlendMode::ColorDodge => Self::ColorDodge,
            BlendMode::Difference => Self::Difference,
        }
    }
}

static SRGB_TO_LINEAR_LUT: LazyLock<[f32; 256]> = LazyLock::new(|| {
    array::from_fn(|index| {
        let channel = u8::try_from(index).expect("LUT index must fit in u8");
        srgb_u8_to_linear(channel)
    })
});
static LINEAR_TO_SRGB_LUT: LazyLock<Vec<u8>> = LazyLock::new(|| {
    (0_u16..=u16::MAX)
        .map(|index| linear_to_srgb_u8(f32::from(index) / LINEAR_ENCODE_LUT_SCALE))
        .collect()
});
static SCREEN_BLEND_LUT: LazyLock<Vec<u8>> = LazyLock::new(|| {
    (0..CHANNEL_PAIR_LUT_SIZE)
        .map(|index| {
            let dst = u8::try_from(index >> 8).expect("LUT high byte must fit in u8");
            let src = u8::try_from(index & 0xff).expect("LUT low byte must fit in u8");
            encode_srgb_channel(screen_blend(
                decode_srgb_channel(dst),
                decode_srgb_channel(src),
            ))
        })
        .collect()
});
static DIFFERENCE_BLEND_LUT: LazyLock<Vec<u8>> = LazyLock::new(|| {
    (0..CHANNEL_PAIR_LUT_SIZE)
        .map(|index| {
            let dst = u8::try_from(index >> 8).expect("LUT high byte must fit in u8");
            let src = u8::try_from(index & 0xff).expect("LUT low byte must fit in u8");
            encode_srgb_channel((decode_srgb_channel(dst) - decode_srgb_channel(src)).abs())
        })
        .collect()
});

pub fn blend_rgba_pixels_in_place(
    target_pixels: &mut [u8],
    source_pixels: &[u8],
    mode: RgbaBlendMode,
    opacity: f32,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }

    match mode {
        RgbaBlendMode::Normal => {
            let len = target_pixels.len().min(source_pixels.len());
            if opacity >= 1.0 {
                let mut offset = 0;
                while offset + 3 < len {
                    let source_alpha_channel = source_pixels[offset + 3];
                    if source_alpha_channel == 0 {
                        offset += 4;
                        continue;
                    }

                    if source_alpha_channel == 255 && target_pixels[offset + 3] == 255 {
                        target_pixels[offset..offset + 4]
                            .copy_from_slice(&source_pixels[offset..offset + 4]);
                        offset += 4;
                        continue;
                    }

                    let blended = blend_rgba_pixel(
                        [
                            target_pixels[offset],
                            target_pixels[offset + 1],
                            target_pixels[offset + 2],
                            target_pixels[offset + 3],
                        ],
                        [
                            source_pixels[offset],
                            source_pixels[offset + 1],
                            source_pixels[offset + 2],
                            source_pixels[offset + 3],
                        ],
                        RgbaBlendMode::Normal,
                        opacity,
                    );
                    target_pixels[offset..offset + 4].copy_from_slice(&blended);
                    offset += 4;
                }
            } else {
                let inverse_alpha = 1.0 - opacity;
                let mut offset = 0;
                while offset + 3 < len {
                    let source_alpha_channel = source_pixels[offset + 3];
                    if source_alpha_channel == 0 {
                        offset += 4;
                        continue;
                    }

                    if source_alpha_channel == 255 && target_pixels[offset + 3] == 255 {
                        target_pixels[offset] = encode_srgb_channel(
                            decode_srgb_channel(target_pixels[offset]).mul_add(
                                inverse_alpha,
                                decode_srgb_channel(source_pixels[offset]) * opacity,
                            ),
                        );
                        target_pixels[offset + 1] = encode_srgb_channel(
                            decode_srgb_channel(target_pixels[offset + 1]).mul_add(
                                inverse_alpha,
                                decode_srgb_channel(source_pixels[offset + 1]) * opacity,
                            ),
                        );
                        target_pixels[offset + 2] = encode_srgb_channel(
                            decode_srgb_channel(target_pixels[offset + 2]).mul_add(
                                inverse_alpha,
                                decode_srgb_channel(source_pixels[offset + 2]) * opacity,
                            ),
                        );
                        offset += 4;
                        continue;
                    }

                    let blended = blend_rgba_pixel(
                        [
                            target_pixels[offset],
                            target_pixels[offset + 1],
                            target_pixels[offset + 2],
                            target_pixels[offset + 3],
                        ],
                        [
                            source_pixels[offset],
                            source_pixels[offset + 1],
                            source_pixels[offset + 2],
                            source_pixels[offset + 3],
                        ],
                        RgbaBlendMode::Normal,
                        opacity,
                    );
                    target_pixels[offset..offset + 4].copy_from_slice(&blended);
                    offset += 4;
                }
            }
        }
        RgbaBlendMode::Screen => {
            blend_screen_rgba_pixels_in_place(target_pixels, source_pixels, opacity);
        }
        RgbaBlendMode::Add
        | RgbaBlendMode::Multiply
        | RgbaBlendMode::Overlay
        | RgbaBlendMode::SoftLight
        | RgbaBlendMode::ColorDodge => {
            for (dst_px, src_px) in target_pixels
                .chunks_exact_mut(4)
                .zip(source_pixels.chunks_exact(4))
            {
                let blended = blend_rgba_pixel(
                    [dst_px[0], dst_px[1], dst_px[2], dst_px[3]],
                    [src_px[0], src_px[1], src_px[2], src_px[3]],
                    mode,
                    opacity,
                );
                dst_px.copy_from_slice(&blended);
            }
        }
        RgbaBlendMode::Difference => {
            blend_difference_rgba_pixels_in_place(target_pixels, source_pixels, opacity);
        }
    }
}

fn blend_screen_rgba_pixels_in_place(target_pixels: &mut [u8], source_pixels: &[u8], opacity: f32) {
    if opacity < 1.0 {
        blend_rgba_pixels_with_reference(
            target_pixels,
            source_pixels,
            RgbaBlendMode::Screen,
            opacity,
        );
        return;
    }

    for (dst_px, src_px) in target_pixels
        .chunks_exact_mut(4)
        .zip(source_pixels.chunks_exact(4))
    {
        if src_px[3] == 0 {
            continue;
        }

        if src_px[3] == 255 && dst_px[3] == 255 {
            dst_px[0] = screen_blend_channel(dst_px[0], src_px[0]);
            dst_px[1] = screen_blend_channel(dst_px[1], src_px[1]);
            dst_px[2] = screen_blend_channel(dst_px[2], src_px[2]);
            continue;
        }

        let blended = blend_rgba_pixel(
            [dst_px[0], dst_px[1], dst_px[2], dst_px[3]],
            [src_px[0], src_px[1], src_px[2], src_px[3]],
            RgbaBlendMode::Screen,
            opacity,
        );
        dst_px.copy_from_slice(&blended);
    }
}

fn blend_difference_rgba_pixels_in_place(
    target_pixels: &mut [u8],
    source_pixels: &[u8],
    opacity: f32,
) {
    if opacity < 1.0 {
        blend_rgba_pixels_with_reference(
            target_pixels,
            source_pixels,
            RgbaBlendMode::Difference,
            opacity,
        );
        return;
    }

    for (dst_px, src_px) in target_pixels
        .chunks_exact_mut(4)
        .zip(source_pixels.chunks_exact(4))
    {
        if src_px[3] == 0 {
            continue;
        }

        if src_px[3] == 255 && dst_px[3] == 255 {
            dst_px[0] = difference_blend_channel(dst_px[0], src_px[0]);
            dst_px[1] = difference_blend_channel(dst_px[1], src_px[1]);
            dst_px[2] = difference_blend_channel(dst_px[2], src_px[2]);
            continue;
        }

        let blended = blend_rgba_pixel(
            [dst_px[0], dst_px[1], dst_px[2], dst_px[3]],
            [src_px[0], src_px[1], src_px[2], src_px[3]],
            RgbaBlendMode::Difference,
            opacity,
        );
        dst_px.copy_from_slice(&blended);
    }
}

fn blend_rgba_pixels_with_reference(
    target_pixels: &mut [u8],
    source_pixels: &[u8],
    mode: RgbaBlendMode,
    opacity: f32,
) {
    for (dst_px, src_px) in target_pixels
        .chunks_exact_mut(4)
        .zip(source_pixels.chunks_exact(4))
    {
        let blended = blend_rgba_pixel(
            [dst_px[0], dst_px[1], dst_px[2], dst_px[3]],
            [src_px[0], src_px[1], src_px[2], src_px[3]],
            mode,
            opacity,
        );
        dst_px.copy_from_slice(&blended);
    }
}

fn screen_blend_channel(dst: u8, src: u8) -> u8 {
    SCREEN_BLEND_LUT[(usize::from(dst) << 8) | usize::from(src)]
}

fn difference_blend_channel(dst: u8, src: u8) -> u8 {
    DIFFERENCE_BLEND_LUT[(usize::from(dst) << 8) | usize::from(src)]
}

pub fn blend_opaque_normal_rgba_pixels_in_place(
    target_pixels: &mut [u8],
    source_pixels: &[u8],
    opacity: f32,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }
    if opacity >= 1.0 {
        blend_rgba_pixels_in_place(target_pixels, source_pixels, RgbaBlendMode::Normal, 1.0);
        return;
    }

    let inverse_alpha = 1.0 - opacity;
    let len = target_pixels.len().min(source_pixels.len());
    let mut offset = 0;
    while offset + 3 < len {
        target_pixels[offset] =
            encode_srgb_channel(decode_srgb_channel(target_pixels[offset]).mul_add(
                inverse_alpha,
                decode_srgb_channel(source_pixels[offset]) * opacity,
            ));
        target_pixels[offset + 1] =
            encode_srgb_channel(decode_srgb_channel(target_pixels[offset + 1]).mul_add(
                inverse_alpha,
                decode_srgb_channel(source_pixels[offset + 1]) * opacity,
            ));
        target_pixels[offset + 2] =
            encode_srgb_channel(decode_srgb_channel(target_pixels[offset + 2]).mul_add(
                inverse_alpha,
                decode_srgb_channel(source_pixels[offset + 2]) * opacity,
            ));
        offset += 4;
    }
}

pub fn apply_layer_adjust_rgba_pixels_in_place(pixels: &mut [u8], adjust: &LayerAdjust) {
    let adjust = adjust.normalized();
    if layer_adjust_is_identity(&adjust) {
        return;
    }

    let hue_shift = adjust.hue_shift / std::f32::consts::TAU;
    let tint_strength = (adjust.tint_strength * adjust.tint[3].clamp(0.0, 1.0)).clamp(0.0, 1.0);
    let contrast_factor = 1.0 + adjust.contrast;
    for pixel in pixels.chunks_exact_mut(4) {
        if pixel[3] == 0 {
            continue;
        }

        let mut red = decode_srgb_channel(pixel[0]) * adjust.brightness;
        let mut green = decode_srgb_channel(pixel[1]) * adjust.brightness;
        let mut blue = decode_srgb_channel(pixel[2]) * adjust.brightness;

        if (adjust.saturation - 1.0).abs() > f32::EPSILON || hue_shift.abs() > f32::EPSILON {
            let (mut hue, saturation, lightness) = rgb_to_hsl(red, green, blue);
            hue = (hue + hue_shift).rem_euclid(1.0);
            let (shifted_red, shifted_green, shifted_blue) = hsl_to_rgb(
                hue,
                (saturation * adjust.saturation).clamp(0.0, 1.0),
                lightness,
            );
            red = shifted_red;
            green = shifted_green;
            blue = shifted_blue;
        }

        if adjust.contrast.abs() > f32::EPSILON {
            red = apply_contrast(red, contrast_factor);
            green = apply_contrast(green, contrast_factor);
            blue = apply_contrast(blue, contrast_factor);
        }

        if tint_strength > 0.0 {
            red = red.mul_add(
                1.0 - tint_strength,
                adjust.tint[0].clamp(0.0, 1.0) * tint_strength,
            );
            green = green.mul_add(
                1.0 - tint_strength,
                adjust.tint[1].clamp(0.0, 1.0) * tint_strength,
            );
            blue = blue.mul_add(
                1.0 - tint_strength,
                adjust.tint[2].clamp(0.0, 1.0) * tint_strength,
            );
        }

        pixel[0] = encode_srgb_channel(red);
        pixel[1] = encode_srgb_channel(green);
        pixel[2] = encode_srgb_channel(blue);
    }
}

#[must_use]
pub fn blend_rgba_pixel(dst: [u8; 4], src: [u8; 4], mode: RgbaBlendMode, opacity: f32) -> [u8; 4] {
    let source_alpha_channel = src[3];
    if source_alpha_channel == 0 || opacity <= 0.0 {
        return dst;
    }

    let source_alpha = alpha_weight(source_alpha_channel, opacity.clamp(0.0, 1.0));
    if source_alpha <= 0.0 {
        return dst;
    }

    let inverse_alpha = 1.0 - source_alpha;
    let dst_red = decode_srgb_channel(dst[0]);
    let dst_green = decode_srgb_channel(dst[1]);
    let dst_blue = decode_srgb_channel(dst[2]);
    let src_red = decode_srgb_channel(src[0]);
    let src_green = decode_srgb_channel(src[1]);
    let src_blue = decode_srgb_channel(src[2]);
    let blend_channel = |dst_channel: f32, src_channel: f32| -> u8 {
        let blended = match mode {
            RgbaBlendMode::Normal => src_channel,
            RgbaBlendMode::Add => (dst_channel + src_channel).min(1.0),
            RgbaBlendMode::Screen => screen_blend(dst_channel, src_channel),
            RgbaBlendMode::Multiply => dst_channel * src_channel,
            RgbaBlendMode::Overlay => {
                if dst_channel < 0.5 {
                    2.0 * dst_channel * src_channel
                } else {
                    1.0 - 2.0 * (1.0 - dst_channel) * (1.0 - src_channel)
                }
            }
            RgbaBlendMode::SoftLight => {
                if src_channel < 0.5 {
                    dst_channel - (1.0 - 2.0 * src_channel) * dst_channel * (1.0 - dst_channel)
                } else {
                    dst_channel + (2.0 * src_channel - 1.0) * (dst_channel.sqrt() - dst_channel)
                }
            }
            RgbaBlendMode::ColorDodge => {
                if src_channel >= 1.0 {
                    1.0
                } else {
                    (dst_channel / (1.0 - src_channel)).min(1.0)
                }
            }
            RgbaBlendMode::Difference => (dst_channel - src_channel).abs(),
        };
        encode_srgb_channel(dst_channel.mul_add(inverse_alpha, blended * source_alpha))
    };
    let alpha = if source_alpha_channel == 255 && dst[3] == 255 {
        255
    } else {
        encode_alpha_channel(composite_alpha(dst[3], source_alpha))
    };

    [
        blend_channel(dst_red, src_red),
        blend_channel(dst_green, src_green),
        blend_channel(dst_blue, src_blue),
        alpha,
    ]
}

#[must_use]
pub fn decode_srgb_channel(channel: u8) -> f32 {
    SRGB_TO_LINEAR_LUT[usize::from(channel)]
}

#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "channel is clamped to the 16-bit LUT domain before rounding to an index"
)]
pub fn encode_srgb_channel(channel: f32) -> u8 {
    let index = (channel.clamp(0.0, 1.0) * LINEAR_ENCODE_LUT_SCALE).round() as u16;
    LINEAR_TO_SRGB_LUT[usize::from(index)]
}

#[must_use]
pub fn alpha_weight(source_alpha: u8, opacity: f32) -> f32 {
    (f32::from(source_alpha) / 255.0) * opacity
}

#[must_use]
pub fn composite_alpha(target_alpha: u8, source_alpha: f32) -> f32 {
    let target_alpha = f32::from(target_alpha) / 255.0;
    (target_alpha + source_alpha - target_alpha * source_alpha).min(1.0)
}

#[must_use]
pub fn encode_alpha_channel(alpha: f32) -> u8 {
    let clamped = alpha.clamp(0.0, 1.0);
    let scaled = (clamped * 255.0).round();
    u8::try_from(scaled as u16).unwrap_or(u8::MAX)
}

#[must_use]
pub fn screen_blend(dst: f32, src: f32) -> f32 {
    1.0 - (1.0 - dst) * (1.0 - src)
}

fn layer_adjust_is_identity(adjust: &LayerAdjust) -> bool {
    (adjust.brightness - 1.0).abs() <= f32::EPSILON
        && (adjust.saturation - 1.0).abs() <= f32::EPSILON
        && adjust.hue_shift.abs() <= f32::EPSILON
        && adjust.tint_strength.abs() <= f32::EPSILON
        && adjust.contrast.abs() <= f32::EPSILON
}

fn apply_contrast(channel: f32, factor: f32) -> f32 {
    (channel - 0.5).mul_add(factor, 0.5)
}

fn rgb_to_hsl(red: f32, green: f32, blue: f32) -> (f32, f32, f32) {
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let lightness = (max + min) * 0.5;
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (0.0, 0.0, lightness);
    }

    let saturation = if lightness > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if (max - red).abs() <= f32::EPSILON {
        ((green - blue) / delta).rem_euclid(6.0)
    } else if (max - green).abs() <= f32::EPSILON {
        ((blue - red) / delta) + 2.0
    } else {
        ((red - green) / delta) + 4.0
    } / 6.0;

    (hue, saturation, lightness)
}

fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> (f32, f32, f32) {
    if saturation <= f32::EPSILON {
        return (lightness, lightness, lightness);
    }

    let q = if lightness < 0.5 {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let p = 2.0 * lightness - q;
    (
        hue_to_rgb(p, q, hue + (1.0 / 3.0)),
        hue_to_rgb(p, q, hue),
        hue_to_rgb(p, q, hue - (1.0 / 3.0)),
    )
}

fn hue_to_rgb(p: f32, q: f32, hue: f32) -> f32 {
    let hue = hue.rem_euclid(1.0);
    if hue < 1.0 / 6.0 {
        p + (q - p) * 6.0 * hue
    } else if hue < 0.5 {
        q
    } else if hue < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - hue) * 6.0
    } else {
        p
    }
}
