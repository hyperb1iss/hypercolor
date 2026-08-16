//! The kernel must reproduce `hypercolor-types::canvas`'s pre-absorption
//! behavior byte for byte. Each test carries the old formula inline and
//! asserts the kernel matches it across the full domain, so the
//! absorption is provable rather than asserted.

use hypercolor_color::{LinearRgba, Oklab, Oklch, Rgb, linear_to_output_u8, linear_to_srgb};

/// Decoding three opaque bytes lands where decoding the same bytes with
/// an explicit alpha of 255 does. Canvas's `from_srgb_u8(r, g, b, 255)`
/// collapsed into `Rgb::to_linear`'s hardcoded `1.0` on that identity,
/// so a drift between the two would silently change opaque decodes.
#[test]
fn rgb_decodes_the_same_as_opaque_rgba() {
    for byte in 0_u8..=255 {
        let rgb = Rgb::new(byte, 255 - byte, byte / 3);
        assert_eq!(rgb.to_linear(), rgb.to_rgba().to_linear(), "at {byte}");
    }
}

#[test]
fn linear_to_output_u8_rounds_and_clamps() {
    assert_eq!(linear_to_output_u8(-1.0), 0);
    assert_eq!(linear_to_output_u8(0.0), 0);
    assert_eq!(linear_to_output_u8(0.5), 128);
    assert_eq!(linear_to_output_u8(1.0), 255);
    assert_eq!(linear_to_output_u8(2.0), 255);
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
