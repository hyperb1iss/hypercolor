use hypercolor_color::PixelBlendMode;
use hypercolor_core::blend_math::{
    blend_rgba_pixel, blend_rgba_pixels_in_place, decode_srgb_channel, encode_srgb_channel,
};
use hypercolor_types::canvas::Rgba;

fn expected_blend(dst: Rgba, src: Rgba, mode: PixelBlendMode, opacity: f32) -> [u8; 4] {
    let dst = dst.to_linear();
    let src = src.to_linear();
    let pixel = src.blend_over(dst, mode, opacity).to_encoded();
    [pixel.r, pixel.g, pixel.b, pixel.a]
}

#[test]
fn single_pixel_blend_matches_canvas_reference() {
    let dst = [255, 0, 0, 255];
    let src = [0, 0, 255, 255];

    for (mode, opacity) in [
        (PixelBlendMode::Normal, 0.25),
        (PixelBlendMode::Add, 1.0),
        (PixelBlendMode::Screen, 1.0),
        (PixelBlendMode::Multiply, 1.0),
        (PixelBlendMode::Overlay, 1.0),
        (PixelBlendMode::SoftLight, 1.0),
        (PixelBlendMode::ColorDodge, 1.0),
        (PixelBlendMode::Difference, 1.0),
    ] {
        assert_eq!(
            blend_rgba_pixel(dst, src, mode, opacity),
            expected_blend(
                Rgba::new(dst[0], dst[1], dst[2], dst[3]),
                Rgba::new(src[0], src[1], src[2], src[3]),
                mode,
                opacity,
            )
        );
    }
}

#[test]
fn slice_blend_updates_pixels_in_place() {
    let mut dst = vec![255, 0, 0, 255, 0, 255, 0, 255];
    let src = vec![0, 0, 255, 255, 255, 255, 255, 128];

    blend_rgba_pixels_in_place(&mut dst, &src, PixelBlendMode::Normal, 0.5);

    assert_eq!(
        &dst[..4],
        &expected_blend(
            Rgba::new(255, 0, 0, 255),
            Rgba::new(0, 0, 255, 255),
            PixelBlendMode::Normal,
            0.5,
        )
    );
    assert_eq!(
        &dst[4..8],
        &expected_blend(
            Rgba::new(0, 255, 0, 255),
            Rgba::new(255, 255, 255, 128),
            PixelBlendMode::Normal,
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
                PixelBlendMode::Difference,
                1.0,
            )
        })
        .collect();

    blend_rgba_pixels_in_place(&mut dst, &src, PixelBlendMode::Difference, 1.0);

    assert_eq!(dst, expected);
}

#[test]
fn screen_slice_blend_matches_single_pixel_reference() {
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
                PixelBlendMode::Screen,
                1.0,
            )
        })
        .collect();

    blend_rgba_pixels_in_place(&mut dst, &src, PixelBlendMode::Screen, 1.0);

    assert_eq!(dst, expected);
}

#[test]
fn opaque_normal_slice_blend_copies_source_at_full_opacity() {
    let mut dst = vec![12, 34, 56, 255, 78, 90, 123, 255];
    let src = vec![210, 180, 140, 255, 1, 2, 3, 255];

    blend_rgba_pixels_in_place(&mut dst, &src, PixelBlendMode::Normal, 1.0);

    assert_eq!(dst, src);
}

/// The compositor's decode is the kernel's table, not a copy of it. A
/// re-introduced local table would drift silently, so every byte is
/// compared.
#[test]
fn decode_channel_is_the_kernel_table_for_every_byte() {
    for byte in 0_u8..=255 {
        assert_eq!(
            decode_srgb_channel(byte),
            hypercolor_color::lut::srgb_u8_to_linear(byte),
            "decode disagrees at {byte}"
        );
    }
}

/// The compositor's encode quantizes to 16 bits before reading the
/// kernel's table. On every one of its own bins the answer is exactly
/// what the kernel returns, which is what makes the table a projection
/// of the kernel rather than a second implementation.
#[test]
fn encode_channel_reads_kernel_entries_at_its_own_quantization() {
    for index in 0_u32..=65_535 {
        #[allow(clippy::cast_precision_loss)]
        let linear = index as f32 / 65_535.0;
        assert_eq!(
            encode_srgb_channel(linear),
            hypercolor_color::lut::linear_to_srgb_u8(linear),
            "encode disagrees at bin {index}"
        );
    }
}

/// The finer quantization is not redundant. Between its own bins the
/// compositor's encode disagrees with a direct kernel call in the dark
/// region, where the sRGB curve is steep enough that a 12-bit bin spans
/// more than one output byte. Collapsing `encode_srgb_channel` into a
/// direct kernel call would brighten near-black composites by one LSB,
/// so this pins the difference rather than leaving it to be discovered.
#[test]
fn sixteen_bit_quantization_is_load_bearing_in_the_dark_region() {
    let near_black = 0.000_122_25_f32;
    assert_eq!(encode_srgb_channel(near_black), 0);
    assert_eq!(hypercolor_color::lut::linear_to_srgb_u8(near_black), 1);
}

/// Encode and decode remain inverses across the byte domain, which is
/// what keeps an opaque normal blend a pure copy.
#[test]
fn encode_decode_roundtrips_every_byte() {
    for byte in 0_u8..=255 {
        assert_eq!(encode_srgb_channel(decode_srgb_channel(byte)), byte);
    }
}
