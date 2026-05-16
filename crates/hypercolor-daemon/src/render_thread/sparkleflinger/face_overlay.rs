use hypercolor_core::blend_math::{
    RgbaBlendMode, blend_rgba_pixels_in_place, decode_srgb_channel, encode_srgb_channel,
    screen_blend,
};
use hypercolor_types::scene::DisplayFaceBlendMode;

pub(super) fn blend_face_overlay_rgba(
    scene_rgba: &mut [u8],
    face_rgba: &[u8],
    blend_mode: DisplayFaceBlendMode,
    opacity: f32,
) {
    match blend_mode {
        DisplayFaceBlendMode::Replace => {
            replace_face_rgba_in_place(scene_rgba, face_rgba, opacity);
        }
        DisplayFaceBlendMode::Tint => {
            blend_face_material_tint_rgba(scene_rgba, face_rgba, opacity);
        }
        DisplayFaceBlendMode::LumaReveal => {
            blend_face_luma_reveal_rgba(scene_rgba, face_rgba, opacity);
        }
        _ => {
            let Some(canvas_blend_mode) = blend_mode.standard_canvas_blend_mode() else {
                return;
            };
            blend_rgba_pixels_in_place(
                scene_rgba,
                face_rgba,
                RgbaBlendMode::from(canvas_blend_mode),
                opacity,
            );
        }
    }

    for pixel in scene_rgba.chunks_exact_mut(4) {
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
