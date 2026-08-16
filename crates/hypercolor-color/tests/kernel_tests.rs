//! Unit coverage for the kernel surfaces the oracle can't judge:
//! hex grammar, the blend kernel, device encoding, scaling, and the
//! wrap/lerp/luma primitives.

#![allow(clippy::float_cmp)] // exact identities are the contract under test

use hypercolor_color::{
    ColorParseError, DevicePixelLayout, Hsl, Hsv, LinearRgba, Oklab, Oklch, PixelBlendMode, Rgb,
    Rgba, wrap_hue,
};

// ── hex grammar ────────────────────────────────────────────────────────────

#[test]
fn hex_rejects_alpha_forms_for_rgb() {
    assert_eq!(Rgb::from_hex("#f80c"), Err(ColorParseError::BadLength(4)));
    assert_eq!(
        Rgb::from_hex("#ff880080"),
        Err(ColorParseError::BadLength(8))
    );
}

#[test]
fn hex_error_variants_are_specific() {
    assert_eq!(Rgb::from_hex(""), Err(ColorParseError::BadLength(0)));
    assert_eq!(Rgb::from_hex("#"), Err(ColorParseError::BadLength(0)));
    assert_eq!(Rgb::from_hex("#ab"), Err(ColorParseError::BadLength(2)));
    assert_eq!(Rgb::from_hex("#ggg"), Err(ColorParseError::BadDigit));
    assert_eq!(Rgba::from_hex("#ff880g08"), Err(ColorParseError::BadDigit));
    // Only one leading `#` is stripped; a second is a bad digit or a bad
    // length depending on where the gate fires — never a success.
    assert!(Rgb::from_hex("##fff").is_err());
    assert!(Rgba::from_hex("##fff").is_err());
}

#[test]
fn hex_roundtrips_through_to_hex() {
    for r in (0..=255u16).step_by(17) {
        for b in (0..=255u16).step_by(51) {
            let rgb = Rgb::new(r as u8, 128, b as u8);
            let parsed = Rgb::from_hex(&rgb.to_hex()).expect("formatted hex parses");
            assert_eq!(parsed, rgb);
        }
    }
}

#[test]
fn hex_srgb_parse_linearizes_after_parsing() {
    let lin = LinearRgba::from_hex_srgb("#808080").expect("valid hex");
    // 0x80 encoded is ~0.216 linear — parse-then-linearize, never the
    // raw byte scaled.
    assert!((lin.r - 0.215_86).abs() < 1e-3, "got {}", lin.r);
    assert_eq!(lin.a, 1.0);
}

// ── blend kernel ───────────────────────────────────────────────────────────

const DST: LinearRgba = LinearRgba::new(0.25, 0.5, 0.75, 1.0);

const ALL_MODES: [PixelBlendMode; 8] = [
    PixelBlendMode::Normal,
    PixelBlendMode::Add,
    PixelBlendMode::Screen,
    PixelBlendMode::Multiply,
    PixelBlendMode::Overlay,
    PixelBlendMode::SoftLight,
    PixelBlendMode::ColorDodge,
    PixelBlendMode::Difference,
];

#[test]
fn blend_at_zero_opacity_is_destination() {
    let src = LinearRgba::new(0.9, 0.1, 0.4, 1.0);
    for mode in ALL_MODES {
        assert_eq!(src.blend_over(DST, mode, 0.0), DST, "{mode:?}");
    }
}

#[test]
fn blend_with_transparent_source_is_destination() {
    let src = LinearRgba::new(0.9, 0.1, 0.4, 0.0);
    for mode in ALL_MODES {
        assert_eq!(src.blend_over(DST, mode, 1.0), DST, "{mode:?}");
    }
}

#[test]
fn normal_blend_at_full_opacity_replaces_color() {
    let src = LinearRgba::new(0.9, 0.1, 0.4, 1.0);
    let out = src.blend_over(DST, PixelBlendMode::Normal, 1.0);
    assert_eq!((out.r, out.g, out.b, out.a), (0.9, 0.1, 0.4, 1.0));
}

#[test]
fn normal_blend_at_half_opacity_is_lerp() {
    let src = LinearRgba::new(1.0, 0.0, 0.0, 1.0);
    let out = src.blend_over(DST, PixelBlendMode::Normal, 0.5);
    assert!((out.r - 0.625).abs() < 1e-6);
    assert!((out.g - 0.25).abs() < 1e-6);
    assert!((out.b - 0.375).abs() < 1e-6);
}

#[test]
fn add_blend_clamps_at_one() {
    let src = LinearRgba::new(1.0, 1.0, 1.0, 1.0);
    let out = src.blend_over(
        LinearRgba::new(0.8, 0.8, 0.8, 1.0),
        PixelBlendMode::Add,
        1.0,
    );
    assert_eq!((out.r, out.g, out.b), (1.0, 1.0, 1.0));
}

#[test]
fn multiply_by_black_is_black() {
    let src = LinearRgba::new(0.0, 0.0, 0.0, 1.0);
    let out = src.blend_over(DST, PixelBlendMode::Multiply, 1.0);
    assert_eq!((out.r, out.g, out.b), (0.0, 0.0, 0.0));
}

#[test]
fn result_alpha_is_source_over_union() {
    let src = LinearRgba::new(0.5, 0.5, 0.5, 0.5);
    let dst = LinearRgba::new(0.0, 0.0, 0.0, 0.5);
    let out = src.blend_over(dst, PixelBlendMode::Normal, 1.0);
    assert!((out.a - 0.75).abs() < 1e-6);
}

// ── device encoding ────────────────────────────────────────────────────────

#[test]
fn encode_orders_channels_per_layout() {
    let px = Rgb::new(1, 2, 3);
    assert_eq!(px.encode(DevicePixelLayout::Rgb).as_slice(), &[1, 2, 3]);
    assert_eq!(px.encode(DevicePixelLayout::Grb).as_slice(), &[2, 1, 3]);
    assert_eq!(px.encode(DevicePixelLayout::Rbg).as_slice(), &[1, 3, 2]);
    assert_eq!(
        px.encode(DevicePixelLayout::RgbwZeroWhite).as_slice(),
        &[1, 2, 3, 0]
    );
}

#[test]
fn channel_counts_match_encoded_lengths() {
    for layout in [
        DevicePixelLayout::Rgb,
        DevicePixelLayout::Grb,
        DevicePixelLayout::Rbg,
        DevicePixelLayout::RgbwZeroWhite,
    ] {
        let encoded = Rgb::WHITE.encode(layout);
        assert_eq!(encoded.len, layout.channel_count(), "{layout:?}");
        assert_eq!(
            encoded.as_slice().len(),
            usize::from(layout.channel_count()),
            "{layout:?}"
        );
    }
}

#[test]
fn scale_rounds_half_away_from_zero() {
    assert_eq!(Rgb::new(255, 255, 255).scale(0.5), Rgb::new(128, 128, 128));
    assert_eq!(Rgb::new(1, 1, 1).scale(0.5), Rgb::new(1, 1, 1));
}

#[test]
fn scale_clamps_both_ends() {
    assert_eq!(Rgb::new(200, 200, 200).scale(2.0), Rgb::WHITE);
    assert_eq!(Rgb::new(100, 100, 100).scale(-1.0), Rgb::BLACK);
}

#[test]
fn scale_nan_factor_is_black() {
    assert_eq!(Rgb::new(100, 100, 100).scale(f32::NAN), Rgb::BLACK);
}

// ── primitives ─────────────────────────────────────────────────────────────

#[test]
fn wrap_hue_never_returns_the_modulus() {
    for h in [-1e-6_f32, -360.0, 0.0, 359.999_97, 360.0, 720.0, -720.0] {
        let wrapped = wrap_hue(h);
        assert!((0.0..360.0).contains(&wrapped), "wrap_hue({h}) = {wrapped}");
    }
    assert_eq!(wrap_hue(360.0), 0.0);
    assert_eq!(wrap_hue(-360.0), 0.0);
}

#[test]
fn lerp_endpoints_are_exact() {
    let a = LinearRgba::new(0.1, 0.2, 0.3, 0.4);
    let b = LinearRgba::new(0.9, 0.8, 0.7, 0.6);
    assert_eq!(a.lerp(b, 0.0), a);
    assert_eq!(a.lerp(b, 1.0), b);
    let mid = a.lerp(b, 0.5);
    assert!((mid.r - 0.5).abs() < 1e-6);
    assert!((mid.a - 0.5).abs() < 1e-6);
}

#[test]
fn luma_of_white_is_one() {
    assert!((LinearRgba::new(1.0, 1.0, 1.0, 1.0).luma() - 1.0).abs() < 1e-6);
    assert!((Rgb::WHITE.luma_encoded() - 1.0).abs() < 1e-6);
    assert_eq!(LinearRgba::new(0.0, 0.0, 0.0, 1.0).luma(), 0.0);
}

#[test]
fn byte_linear_conversions_roundtrip() {
    let rgba = Rgba::new(200, 100, 50, 128);
    let back = rgba.to_linear().to_encoded();
    assert_eq!(back, rgba);
}

#[test]
fn oklch_polar_roundtrip() {
    let lab = Oklab::new(0.7, 0.1, -0.05, 0.8);
    let back = Oklch::from_oklab(lab).to_oklab();
    assert!((back.l - lab.l).abs() < 1e-6);
    assert!((back.a - lab.a).abs() < 1e-6);
    assert!((back.b - lab.b).abs() < 1e-6);
    assert_eq!(back.alpha, lab.alpha);
}

#[test]
fn oklch_hue_lands_in_range() {
    let lch = Oklch::from_oklab(Oklab::new(0.5, -0.1, -0.1, 1.0));
    assert!((0.0..360.0).contains(&lch.h));
}

#[test]
fn oklch_wraps_hue_at_entry() {
    let at_zero = Oklch::new(0.7, 0.15, 0.0, 1.0).to_oklab();
    let at_full = Oklch::new(0.7, 0.15, 360.0, 1.0).to_oklab();
    assert_eq!((at_zero.a, at_zero.b), (at_full.a, at_full.b));
}

#[test]
fn oklch_lerp_takes_the_short_hue_arc() {
    let a = Oklch::new(0.7, 0.15, 350.0, 1.0);
    let b = Oklch::new(0.7, 0.15, 10.0, 1.0);
    let mid = a.lerp(b, 0.5);
    assert!(
        mid.h < 1e-4 || mid.h > 359.999,
        "midpoint of 350°→10° must sit at 0°, got {}",
        mid.h
    );
    assert_eq!(a.lerp(b, 0.0).h, 350.0);
    assert_eq!(a.lerp(b, 1.0).h, 10.0);
}

#[test]
fn oklch_composition_paths_agree() {
    let lin = LinearRgba::new(0.4, 0.2, 0.6, 0.9);
    let via_method = lin.to_oklch();
    let via_steps = Oklch::from_oklab(lin.to_oklab());
    assert_eq!(via_method, via_steps);
    let back = via_method.to_linear();
    assert!((back.r - lin.r).abs() < 1e-3);
    assert_eq!(back.a, lin.a);
}

#[test]
fn hsv_hsl_byte_roundtrip_within_one_lsb() {
    for r in (0..=255u16).step_by(51) {
        for g in (0..=255u16).step_by(51) {
            for b in (0..=255u16).step_by(51) {
                let rgb = Rgb::new(r as u8, g as u8, b as u8);
                let via_hsv = Hsv::from_rgb(rgb).to_rgb();
                let via_hsl = Hsl::from_rgb(rgb).to_rgb();
                for (name, back) in [("hsv", via_hsv), ("hsl", via_hsl)] {
                    assert!(
                        rgb.r.abs_diff(back.r) <= 1
                            && rgb.g.abs_diff(back.g) <= 1
                            && rgb.b.abs_diff(back.b) <= 1,
                        "{name} roundtrip drifted: {rgb:?} -> {back:?}"
                    );
                }
            }
        }
    }
}
