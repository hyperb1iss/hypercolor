use std::array;
use std::sync::LazyLock;

use crate::types::canvas::{BlendMode, linear_to_srgb_u8, srgb_u8_to_linear};

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
