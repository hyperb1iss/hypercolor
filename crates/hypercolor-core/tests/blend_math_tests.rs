use hypercolor_core::blend_math::{RgbaBlendMode, blend_rgba_pixel, blend_rgba_pixels_in_place};
use hypercolor_types::canvas::{BlendMode, Rgba, RgbaF32};

fn expected_blend(dst: Rgba, src: Rgba, mode: BlendMode, opacity: f32) -> [u8; 4] {
    let dst = dst.to_linear_f32();
    let src = src.to_linear_f32();
    let blended = mode.blend(
        [dst.r, dst.g, dst.b, dst.a],
        [src.r, src.g, src.b, src.a],
        opacity,
    );
    let pixel = RgbaF32::new(blended[0], blended[1], blended[2], blended[3]).to_srgba();
    [pixel.r, pixel.g, pixel.b, pixel.a]
}

#[test]
fn single_pixel_blend_matches_canvas_reference() {
    let dst = [255, 0, 0, 255];
    let src = [0, 0, 255, 255];

    for (rgba_mode, canvas_mode, opacity) in [
        (RgbaBlendMode::Normal, BlendMode::Normal, 0.25),
        (RgbaBlendMode::Add, BlendMode::Add, 1.0),
        (RgbaBlendMode::Screen, BlendMode::Screen, 1.0),
        (RgbaBlendMode::Multiply, BlendMode::Multiply, 1.0),
        (RgbaBlendMode::Overlay, BlendMode::Overlay, 1.0),
        (RgbaBlendMode::SoftLight, BlendMode::SoftLight, 1.0),
        (RgbaBlendMode::ColorDodge, BlendMode::ColorDodge, 1.0),
        (RgbaBlendMode::Difference, BlendMode::Difference, 1.0),
    ] {
        assert_eq!(
            blend_rgba_pixel(dst, src, rgba_mode, opacity),
            expected_blend(
                Rgba::new(dst[0], dst[1], dst[2], dst[3]),
                Rgba::new(src[0], src[1], src[2], src[3]),
                canvas_mode,
                opacity,
            )
        );
    }
}

#[test]
fn slice_blend_updates_pixels_in_place() {
    let mut dst = vec![255, 0, 0, 255, 0, 255, 0, 255];
    let src = vec![0, 0, 255, 255, 255, 255, 255, 128];

    blend_rgba_pixels_in_place(&mut dst, &src, RgbaBlendMode::Normal, 0.5);

    assert_eq!(
        &dst[..4],
        &expected_blend(
            Rgba::new(255, 0, 0, 255),
            Rgba::new(0, 0, 255, 255),
            BlendMode::Normal,
            0.5,
        )
    );
    assert_eq!(
        &dst[4..8],
        &expected_blend(
            Rgba::new(0, 255, 0, 255),
            Rgba::new(255, 255, 255, 128),
            BlendMode::Normal,
            0.5,
        )
    );
}

#[test]
fn difference_slice_blend_matches_single_pixel_reference() {
    let mut dst = vec![
        12, 34, 56, 255, 210, 180, 140, 255, 24, 48, 96, 128, 2, 4, 8, 255,
    ];
    let src = vec![
        90, 80, 70, 255, 1, 2, 3, 128, 220, 180, 140, 255, 200, 100, 50, 0,
    ];
    let expected: Vec<u8> = dst
        .chunks_exact(4)
        .zip(src.chunks_exact(4))
        .flat_map(|(dst_px, src_px)| {
            blend_rgba_pixel(
                [dst_px[0], dst_px[1], dst_px[2], dst_px[3]],
                [src_px[0], src_px[1], src_px[2], src_px[3]],
                RgbaBlendMode::Difference,
                1.0,
            )
        })
        .collect();

    blend_rgba_pixels_in_place(&mut dst, &src, RgbaBlendMode::Difference, 1.0);

    assert_eq!(dst, expected);
}

#[test]
fn opaque_normal_slice_blend_copies_source_at_full_opacity() {
    let mut dst = vec![12, 34, 56, 255, 78, 90, 123, 255];
    let src = vec![210, 180, 140, 255, 1, 2, 3, 255];

    blend_rgba_pixels_in_place(&mut dst, &src, RgbaBlendMode::Normal, 1.0);

    assert_eq!(dst, src);
}
