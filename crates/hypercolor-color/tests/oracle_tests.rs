//! Property tests against the `palette` crate as a reference oracle
//! (Spec 76 §1: dev-dependency only — never a runtime dep).
//!
//! Every kernel must match reference math within tolerance across dense
//! sweeps. Byte outputs may differ by at most one LSB (rounding at exact
//! half-steps is implementation-defined between libraries).

use hypercolor_color::{Hsl, Hsv, LinearRgba, Rgb, lut, srgb_to_linear};
use palette::FromColor;

type PalHsv = palette::Hsv<palette::encoding::Srgb, f32>;
type PalHsl = palette::Hsl<palette::encoding::Srgb, f32>;
type PalSrgb = palette::Srgb<f32>;
type PalLin = palette::LinSrgb<f32>;
type PalOklab = palette::Oklab<f32>;

fn assert_byte_close(ours: u8, oracle: u8, ctx: &str) {
    assert!(
        i16::from(ours).abs_diff(i16::from(oracle)) <= 1,
        "{ctx}: ours={ours}, oracle={oracle}"
    );
}

fn assert_close(ours: f32, oracle: f32, tol: f32, ctx: &str) {
    assert!(
        (ours - oracle).abs() <= tol,
        "{ctx}: ours={ours}, oracle={oracle}, tol={tol}"
    );
}

fn hue_distance(a: f32, b: f32) -> f32 {
    let diff = (a - b).abs() % 360.0;
    diff.min(360.0 - diff)
}

const UNIT_STEPS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

#[test]
fn hsv_to_rgb_matches_palette() {
    for h_tenths in (0..3600).step_by(5) {
        let h = h_tenths as f32 / 10.0;
        for s in UNIT_STEPS {
            for v in UNIT_STEPS {
                let ours = Hsv::new(h, s, v).to_rgb();
                let oracle: PalSrgb = PalSrgb::from_color(PalHsv::new(h, s, v));
                let oracle = oracle.into_format::<u8>();
                let ctx = format!("hsv({h}, {s}, {v})");
                assert_byte_close(ours.r, oracle.red, &ctx);
                assert_byte_close(ours.g, oracle.green, &ctx);
                assert_byte_close(ours.b, oracle.blue, &ctx);
            }
        }
    }
}

#[test]
fn hsl_to_rgb_matches_palette() {
    for h_tenths in (0..3600).step_by(5) {
        let h = h_tenths as f32 / 10.0;
        for s in UNIT_STEPS {
            for l in UNIT_STEPS {
                let ours = Hsl::new(h, s, l).to_rgb();
                let oracle: PalSrgb = PalSrgb::from_color(PalHsl::new(h, s, l));
                let oracle = oracle.into_format::<u8>();
                let ctx = format!("hsl({h}, {s}, {l})");
                assert_byte_close(ours.r, oracle.red, &ctx);
                assert_byte_close(ours.g, oracle.green, &ctx);
                assert_byte_close(ours.b, oracle.blue, &ctx);
            }
        }
    }
}

#[test]
fn rgb_to_hsv_matches_palette() {
    for r in (0..=255).step_by(15) {
        for g in (0..=255).step_by(15) {
            for b in (0..=255).step_by(15) {
                let ours = Hsv::from_rgb(Rgb::new(r as u8, g as u8, b as u8));
                let oracle = PalHsv::from_color(
                    palette::Srgb::<u8>::new(r as u8, g as u8, b as u8).into_format::<f32>(),
                );
                let ctx = format!("rgb({r}, {g}, {b}) -> hsv");
                // A gray pixel has no defined hue; both sides return an
                // arbitrary angle there, so only compare when saturated.
                if ours.s > 1e-6 && oracle.saturation > 1e-6 {
                    assert!(
                        hue_distance(ours.h, oracle.hue.into_positive_degrees()) <= 0.05,
                        "{ctx}: hue ours={}, oracle={}",
                        ours.h,
                        oracle.hue.into_positive_degrees()
                    );
                }
                assert_close(ours.s, oracle.saturation, 1e-4, &ctx);
                assert_close(ours.v, oracle.value, 1e-4, &ctx);
            }
        }
    }
}

#[test]
fn rgb_to_hsl_matches_palette() {
    for r in (0..=255).step_by(15) {
        for g in (0..=255).step_by(15) {
            for b in (0..=255).step_by(15) {
                let ours = Hsl::from_rgb(Rgb::new(r as u8, g as u8, b as u8));
                let oracle = PalHsl::from_color(
                    palette::Srgb::<u8>::new(r as u8, g as u8, b as u8).into_format::<f32>(),
                );
                let ctx = format!("rgb({r}, {g}, {b}) -> hsl");
                if ours.s > 1e-6 && oracle.saturation > 1e-6 {
                    assert!(
                        hue_distance(ours.h, oracle.hue.into_positive_degrees()) <= 0.05,
                        "{ctx}: hue ours={}, oracle={}",
                        ours.h,
                        oracle.hue.into_positive_degrees()
                    );
                }
                assert_close(ours.s, oracle.saturation, 1e-4, &ctx);
                assert_close(ours.l, oracle.lightness, 1e-4, &ctx);
            }
        }
    }
}

#[test]
fn srgb_transfer_matches_palette_for_every_byte() {
    for byte in 0..=255u8 {
        let encoded = f32::from(byte) / 255.0;
        let oracle: PalLin = PalSrgb::new(encoded, 0.0, 0.0).into_linear();
        let ctx = format!("byte {byte}");
        assert_close(srgb_to_linear(encoded), oracle.red, 1e-6, &ctx);
        assert_close(lut::srgb_u8_to_linear(byte), oracle.red, 1e-6, &ctx);
    }
}

#[test]
fn transfer_lut_roundtrips_every_byte_exactly() {
    for byte in 0..=255u8 {
        assert_eq!(
            lut::linear_to_srgb_u8(lut::srgb_u8_to_linear(byte)),
            byte,
            "byte {byte} did not round-trip"
        );
    }
}

#[test]
fn oklab_matches_palette() {
    for r in (0..=255).step_by(51) {
        for g in (0..=255).step_by(51) {
            for b in (0..=255).step_by(51) {
                let lin = Rgb::new(r as u8, g as u8, b as u8).to_linear();
                let ours = lin.to_oklab();
                let oracle = PalOklab::from_color(PalLin::new(lin.r, lin.g, lin.b));
                let ctx = format!("rgb({r}, {g}, {b}) -> oklab");
                assert_close(ours.l, oracle.l, 2e-3, &ctx);
                assert_close(ours.a, oracle.a, 2e-3, &ctx);
                assert_close(ours.b, oracle.b, 2e-3, &ctx);
            }
        }
    }
}

#[test]
fn oklab_roundtrip_preserves_color_and_alpha() {
    for r in (0..=255).step_by(51) {
        for g in (0..=255).step_by(51) {
            for b in (0..=255).step_by(51) {
                for alpha in UNIT_STEPS {
                    let lin = LinearRgba {
                        a: alpha,
                        ..Rgb::new(r as u8, g as u8, b as u8).to_linear()
                    };
                    let back = lin.to_oklab().to_linear();
                    let ctx = format!("rgb({r}, {g}, {b}, a={alpha})");
                    assert_close(back.r, lin.r, 1e-3, &ctx);
                    assert_close(back.g, lin.g, 1e-3, &ctx);
                    assert_close(back.b, lin.b, 1e-3, &ctx);
                    assert!(
                        (back.a - alpha).abs() < f32::EPSILON,
                        "{ctx}: alpha drifted"
                    );
                }
            }
        }
    }
}
