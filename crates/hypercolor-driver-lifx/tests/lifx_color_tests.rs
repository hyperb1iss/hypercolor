//! RGB to HSBK conversion (Spec 88 §8).

use hypercolor_driver_lifx::color::{
    DEFAULT_WHITE_KELVIN, Hsbk, HsbkEncoder, clamp_kelvin, hue_degrees_to_u16, unit_to_u16,
};

fn color() -> HsbkEncoder {
    HsbkEncoder::new(DEFAULT_WHITE_KELVIN, true)
}

#[test]
fn hue_uses_the_documented_65536_scale() {
    assert_eq!(hue_degrees_to_u16(0.0), 0);
    assert_eq!(hue_degrees_to_u16(120.0), 21_845);
    assert_eq!(hue_degrees_to_u16(240.0), 43_691);
    assert_eq!(hue_degrees_to_u16(360.0), 0);
    assert_eq!(hue_degrees_to_u16(-120.0), 43_691);
    assert_eq!(unit_to_u16(1.0), 65_535);
    assert_eq!(unit_to_u16(0.5), 32_768);
    assert_eq!(unit_to_u16(2.0), 65_535);
}

#[test]
fn primaries_are_fully_saturated_at_full_brightness() {
    let encoder = color();
    assert_eq!(
        encoder.encode([255, 0, 0]),
        Hsbk::new(0, 65_535, 65_535, 6_500)
    );
    assert_eq!(
        encoder.encode([0, 255, 0]),
        Hsbk::new(21_845, 65_535, 65_535, 6_500)
    );
    assert_eq!(
        encoder.encode([0, 0, 255]),
        Hsbk::new(43_691, 65_535, 65_535, 6_500)
    );
}

#[test]
fn any_color_with_a_dark_channel_stays_fully_saturated() {
    let encoder = color();
    for rgb in [[255, 128, 0], [10, 200, 0], [0, 40, 90], [3, 0, 1]] {
        assert_eq!(encoder.encode(rgb).saturation, 65_535, "{rgb:?}");
    }
}

#[test]
fn greys_and_whites_are_unsaturated_at_the_white_point() {
    let encoder = color();
    assert_eq!(
        encoder.encode([255, 255, 255]),
        Hsbk::new(0, 0, 65_535, 6_500)
    );
    let grey = encoder.encode([128, 128, 128]);
    assert_eq!(grey.saturation, 0);
    assert_eq!(grey.brightness, 128 * 257);
}

#[test]
fn black_is_zero_brightness() {
    assert_eq!(color().encode([0, 0, 0]), Hsbk::new(0, 0, 0, 6_500));
}

#[test]
fn pastels_keep_linear_light_saturation() {
    // sRGB (255, 128, 128) is half saturated in encoded HSV, but most of its
    // light is red: 1 - 0.216 / 1.0 in linear light.
    let pink = color().encode([255, 128, 128]);
    let saturation = f32::from(pink.saturation) / 65_535.0;
    assert!(
        (saturation - 0.784).abs() < 0.01,
        "saturation {saturation} should follow linear light"
    );
    assert!(saturation > 128.0 / 255.0);
    assert_eq!(pink.hue, 0);
    assert_eq!(pink.brightness, 65_535);
}

#[test]
fn hue_follows_the_encoded_hsv_wheel() {
    // Orange (255, 128, 0) sits at 30 degrees on the encoded HSV wheel.
    let orange = color().encode([255, 128, 0]);
    let degrees = f32::from(orange.hue) * 360.0 / 65_536.0;
    assert!((degrees - 30.1).abs() < 0.2, "hue {degrees}");
}

#[test]
fn brightness_scale_applies_to_hsbk_brightness_only() {
    let mut encoder = color();
    encoder.brightness = 128;
    let dimmed = encoder.encode([0, 0, 255]);
    assert_eq!(dimmed.hue, 43_691);
    assert_eq!(dimmed.saturation, 65_535);
    assert_eq!(dimmed.brightness, 32_896);

    encoder.brightness = 0;
    assert_eq!(encoder.encode([255, 255, 255]).brightness, 0);
}

#[test]
fn white_only_lights_get_luminance_brightness() {
    let encoder = HsbkEncoder::new(2_700, false);
    let blue = encoder.encode([0, 0, 255]);
    assert_eq!(blue.saturation, 0);
    assert_eq!(blue.hue, 0);
    assert_eq!(blue.kelvin, 2_700);
    // Blue carries about 7 percent of white's luminance.
    let white = encoder.encode([255, 255, 255]);
    assert_eq!(white.brightness, 65_535);
    assert!(blue.brightness < white.brightness / 2, "{blue:?}");
}

#[test]
fn kelvin_clamps_into_the_product_range() {
    assert_eq!(clamp_kelvin(6_500, (2_500, 9_000)), 6_500);
    assert_eq!(clamp_kelvin(6_500, (2_700, 2_700)), 2_700);
    assert_eq!(clamp_kelvin(1_500, (2_500, 9_000)), 2_500);
    assert_eq!(clamp_kelvin(9_000, (6_500, 2_200)), 6_500);
}
