//! The compat surface must reproduce `hypercolor-types::canvas`'s old
//! behavior byte for byte. Each test carries the pre-absorption formula
//! inline and asserts the kernel matches it across the full domain, so
//! the absorption is provable rather than asserted.

#![allow(deprecated)]

use hypercolor_color::{
    LinearRgba, Oklab, Oklch, Rgba, linear_srgb_to_oklab, linear_to_output_u8, linear_to_srgb,
    oklab_to_linear_srgb,
};

/// Every byte quadruple round-trips through the direct scale exactly as
/// canvas's `Rgba::to_f32` did: divide by 255, no transfer decode.
#[test]
fn to_f32_direct_is_direct_scale_over_every_byte() {
    for byte in 0_u8..=255 {
        let color = Rgba::new(byte, byte, byte, byte);
        let expected = f32::from(byte) / 255.0;
        let actual = color.to_f32_direct();
        assert_eq!(actual.r, expected, "red at {byte}");
        assert_eq!(actual.g, expected, "green at {byte}");
        assert_eq!(actual.b, expected, "blue at {byte}");
        assert_eq!(actual.a, expected, "alpha at {byte}");
    }
}

/// `to_f32_direct` deliberately differs from `to_linear` — the `direct`
/// in the name is the whole point. Anything but a mismatch here means
/// one of the two decodes when it should not.
#[test]
fn to_f32_direct_and_to_linear_disagree_off_the_endpoints() {
    let mid = Rgba::new(128, 128, 128, 255);
    assert_ne!(mid.to_f32_direct().r, mid.to_linear().r);
}

#[test]
fn to_linear_f32_matches_to_linear() {
    for byte in 0_u8..=255 {
        let color = Rgba::new(byte, 255 - byte, byte / 2, byte);
        assert_eq!(color.to_linear_f32(), color.to_linear());
    }
}

#[test]
fn from_srgb_u8_matches_rgba_to_linear() {
    for byte in 0_u8..=255 {
        let color = Rgba::new(byte, 255 - byte, byte / 3, byte);
        assert_eq!(
            LinearRgba::from_srgb_u8(color.r, color.g, color.b, color.a),
            color.to_linear()
        );
    }
}

#[test]
fn to_srgb_u8_and_to_srgba_match_to_encoded() {
    for step in 0_u32..=512 {
        #[allow(clippy::cast_precision_loss)]
        let unit = step as f32 / 512.0;
        let color = LinearRgba::new(unit, 1.0 - unit, unit * 0.5, unit);
        let encoded = color.to_encoded();
        assert_eq!(
            color.to_srgb_u8(),
            [encoded.r, encoded.g, encoded.b, encoded.a]
        );
        assert_eq!(color.to_srgba(), encoded);
    }
}

/// canvas's `RgbaF32::to_rgba` truncated after scaling; the kernel's
/// `to_encoded` rounds after a transfer encode. Both behaviors survive
/// under their own names.
#[test]
fn to_bytes_direct_is_truncating_direct_scale() {
    for step in 0_u32..=1024 {
        #[allow(clippy::cast_precision_loss)]
        let unit = step as f32 / 1024.0;
        let color = LinearRgba::new(unit, unit, unit, unit);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::as_conversions,
            reason = "reproducing the pre-absorption formula verbatim"
        )]
        let expected = (unit * 255.0).clamp(0.0, 255.0) as u8;
        assert_eq!(color.to_bytes_direct().r, expected, "at unit {unit}");
    }
}

#[test]
fn to_bytes_direct_clamps_out_of_range_channels() {
    let out_of_range = LinearRgba::new(-0.5, 1.5, 0.0, 2.0);
    assert_eq!(out_of_range.to_bytes_direct(), Rgba::new(0, 255, 0, 255));
}

#[test]
fn linear_to_output_u8_rounds_and_clamps() {
    assert_eq!(linear_to_output_u8(-1.0), 0);
    assert_eq!(linear_to_output_u8(0.0), 0);
    assert_eq!(linear_to_output_u8(0.5), 128);
    assert_eq!(linear_to_output_u8(1.0), 255);
    assert_eq!(linear_to_output_u8(2.0), 255);
}

/// The free-function Oklab pair canvas exported publicly is the same
/// transform as the method pair, argument for argument.
#[test]
fn oklab_free_functions_match_the_methods() {
    for step in 0_u32..=64 {
        #[allow(clippy::cast_precision_loss)]
        let unit = step as f32 / 64.0;
        let color = LinearRgba::new(unit, 1.0 - unit, unit * unit, 0.75);
        let lab = linear_srgb_to_oklab(color.r, color.g, color.b, color.a);
        assert_eq!(lab, color.to_oklab());
        assert_eq!(oklab_to_linear_srgb(lab), lab.to_linear());
        assert_eq!(lab.to_linear_srgb(), lab.to_linear());
    }
}

#[test]
fn oklch_to_linear_srgb_matches_to_linear() {
    let lch = Oklch::new(0.7, 0.15, 210.0, 0.5);
    assert_eq!(lch.to_linear_srgb(), lch.to_linear());
}

/// Alpha survives the full perceptual round trip untouched — the
/// property Spec 76 §1.2 calls out as previously regressed.
#[test]
fn alpha_survives_the_oklab_round_trip() {
    let color = LinearRgba::new(0.2, 0.6, 0.9, 0.375);
    let back = color.to_oklab().to_linear();
    assert_eq!(back.a, color.a);
    let lab = Oklab::new(0.5, 0.1, -0.1, 0.25);
    assert_eq!(Oklch::from_oklab(lab).alpha, 0.25);
}

/// `RgbaF32` is the same type, not a conversion.
#[test]
fn rgba_f32_alias_is_linear_rgba() {
    let aliased: hypercolor_color::RgbaF32 = LinearRgba::new(0.1, 0.2, 0.3, 1.0);
    assert_eq!(aliased, LinearRgba::new(0.1, 0.2, 0.3, 1.0));
}

/// The scalar transfer function and the 4096-bin LUT agree on every
/// byte-representable input, which is what lets hex parsing and canvas
/// sampling share one table.
#[test]
fn scalar_and_lut_encode_agree_on_byte_inputs() {
    for byte in 0_u8..=255 {
        let linear = hypercolor_color::lut::srgb_u8_to_linear(byte);
        assert_eq!(hypercolor_color::lut::linear_to_srgb_u8(linear), byte);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::as_conversions,
            reason = "reproducing the pre-absorption scalar formula verbatim"
        )]
        let scalar = (linear_to_srgb(linear) * 255.0).round().clamp(0.0, 255.0) as u8;
        assert_eq!(scalar, byte, "scalar encode disagrees at {byte}");
    }
}
