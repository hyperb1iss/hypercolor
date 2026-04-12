use std::array;
use std::sync::LazyLock;

use hypercolor_types::overlay::OverlayBlendMode;

use crate::types::canvas::{linear_to_srgb_u8, srgb_u8_to_linear};

const LINEAR_ENCODE_LUT_SCALE: f32 = 65_535.0;

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

pub fn blend_rgba_pixels_in_place(
    target_pixels: &mut [u8],
    source_pixels: &[u8],
    mode: OverlayBlendMode,
    opacity: f32,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }

    match mode {
        OverlayBlendMode::Normal => {
            for (dst_px, src_px) in target_pixels
                .chunks_exact_mut(4)
                .zip(source_pixels.chunks_exact(4))
            {
                let source_alpha_channel = src_px[3];
                if source_alpha_channel == 0 {
                    continue;
                }

                if source_alpha_channel == 255 && dst_px[3] == 255 {
                    if opacity >= 1.0 {
                        dst_px.copy_from_slice(src_px);
                        continue;
                    }

                    let inverse_alpha = 1.0 - opacity;
                    dst_px[0] = encode_srgb_channel(
                        decode_srgb_channel(dst_px[0])
                            .mul_add(inverse_alpha, decode_srgb_channel(src_px[0]) * opacity),
                    );
                    dst_px[1] = encode_srgb_channel(
                        decode_srgb_channel(dst_px[1])
                            .mul_add(inverse_alpha, decode_srgb_channel(src_px[1]) * opacity),
                    );
                    dst_px[2] = encode_srgb_channel(
                        decode_srgb_channel(dst_px[2])
                            .mul_add(inverse_alpha, decode_srgb_channel(src_px[2]) * opacity),
                    );
                    continue;
                }

                let blended = blend_rgba_pixel(
                    [dst_px[0], dst_px[1], dst_px[2], dst_px[3]],
                    [src_px[0], src_px[1], src_px[2], src_px[3]],
                    OverlayBlendMode::Normal,
                    opacity,
                );
                dst_px.copy_from_slice(&blended);
            }
        }
        OverlayBlendMode::Add | OverlayBlendMode::Screen => {
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
    }
}

#[must_use]
pub fn blend_rgba_pixel(
    dst: [u8; 4],
    src: [u8; 4],
    mode: OverlayBlendMode,
    opacity: f32,
) -> [u8; 4] {
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
            OverlayBlendMode::Normal => src_channel,
            OverlayBlendMode::Add => (dst_channel + src_channel).min(1.0),
            OverlayBlendMode::Screen => screen_blend(dst_channel, src_channel),
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
