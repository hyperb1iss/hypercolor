//! Comprehensive tests for canvas, color, blend mode, and color space types.

use hypercolor_types::canvas::{
    BYTES_PER_PIXEL, BlendMode, Canvas, ColorFormat, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH,
    Oklab, Oklch, Rgb, Rgba, RgbaF32, SamplingMethod, linear_srgb_to_oklab, linear_to_srgb,
    oklab_to_linear_srgb, srgb_to_linear,
};

// ── Rgba ───────────────────────────────────────────────────────────────────

#[test]
fn rgba_constants() {
    assert_eq!(Rgba::BLACK, Rgba::new(0, 0, 0, 255));
    assert_eq!(Rgba::WHITE, Rgba::new(255, 255, 255, 255));
    assert_eq!(Rgba::TRANSPARENT, Rgba::new(0, 0, 0, 0));
}

#[test]
fn rgba_default_is_black() {
    assert_eq!(Rgba::default(), Rgba::BLACK);
}

#[test]
fn rgba_to_f32_roundtrip() {
    let original = Rgba::new(128, 64, 200, 255);
    let float = original.to_f32();
    let back = float.to_rgba();
    assert_eq!(original, back);
}

#[test]
fn rgba_to_f32_boundaries() {
    let black = Rgba::BLACK.to_f32();
    assert!((black.r).abs() < f32::EPSILON);
    assert!((black.g).abs() < f32::EPSILON);
    assert!((black.b).abs() < f32::EPSILON);
    assert!((black.a - 1.0).abs() < f32::EPSILON);

    let white = Rgba::WHITE.to_f32();
    assert!((white.r - 1.0).abs() < f32::EPSILON);
    assert!((white.g - 1.0).abs() < f32::EPSILON);
    assert!((white.b - 1.0).abs() < f32::EPSILON);
}

#[test]
fn rgba_to_rgb() {
    let pixel = Rgba::new(100, 150, 200, 128);
    let rgb = pixel.to_rgb();
    assert_eq!(rgb, Rgb::new(100, 150, 200));
}

#[test]
fn rgba_serde_roundtrip() {
    let color = Rgba::new(42, 128, 255, 200);
    let json = serde_json::to_string(&color).expect("serialize");
    let back: Rgba = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(color, back);
}

// ── Rgb ────────────────────────────────────────────────────────────────────

#[test]
fn rgb_to_rgba() {
    let rgb = Rgb::new(100, 150, 200);
    let rgba = rgb.to_rgba();
    assert_eq!(rgba, Rgba::new(100, 150, 200, 255));
}

#[test]
fn rgb_default_is_zero() {
    let d = Rgb::default();
    assert_eq!(d, Rgb::new(0, 0, 0));
}

// ── RgbaF32 / Color ────────────────────────────────────────────────────────

#[test]
fn rgbaf32_default_is_opaque_black() {
    let c = RgbaF32::default();
    assert!((c.r).abs() < f32::EPSILON);
    assert!((c.g).abs() < f32::EPSILON);
    assert!((c.b).abs() < f32::EPSILON);
    assert!((c.a - 1.0).abs() < f32::EPSILON);
}

#[test]
fn rgbaf32_lerp_midpoint() {
    let a = RgbaF32::new(0.0, 0.0, 0.0, 1.0);
    let b = RgbaF32::new(1.0, 1.0, 1.0, 1.0);
    let mid = RgbaF32::lerp(&a, &b, 0.5);
    assert!((mid.r - 0.5).abs() < f32::EPSILON);
    assert!((mid.g - 0.5).abs() < f32::EPSILON);
    assert!((mid.b - 0.5).abs() < f32::EPSILON);
}

#[test]
fn rgbaf32_lerp_endpoints() {
    let a = RgbaF32::new(0.2, 0.4, 0.6, 0.8);
    let b = RgbaF32::new(0.8, 0.6, 0.4, 0.2);

    let at_zero = RgbaF32::lerp(&a, &b, 0.0);
    assert!((at_zero.r - a.r).abs() < f32::EPSILON);

    let at_one = RgbaF32::lerp(&a, &b, 1.0);
    assert!((at_one.r - b.r).abs() < f32::EPSILON);
}

#[test]
fn rgbaf32_to_rgba_clamps() {
    let oob = RgbaF32::new(1.5, -0.5, 0.5, 2.0);
    let clamped = oob.to_rgba();
    assert_eq!(clamped.r, 255);
    assert_eq!(clamped.g, 0);
    assert_eq!(clamped.b, 127); // 0.5 * 255.0 = 127.5, truncated to 127 by `as u8`
    assert_eq!(clamped.a, 255);
}

#[test]
fn rgbaf32_serde_roundtrip() {
    let color = RgbaF32::new(0.123, 0.456, 0.789, 1.0);
    let json = serde_json::to_string(&color).expect("serialize");
    let back: RgbaF32 = serde_json::from_str(&json).expect("deserialize");
    assert!((color.r - back.r).abs() < f32::EPSILON);
    assert!((color.g - back.g).abs() < f32::EPSILON);
    assert!((color.b - back.b).abs() < f32::EPSILON);
}

// ── sRGB Transfer Functions ────────────────────────────────────────────────

#[test]
fn srgb_roundtrip() {
    for i in 0..=255u16 {
        let srgb = f32::from(i) / 255.0;
        let linear = srgb_to_linear(srgb);
        let back = linear_to_srgb(linear);
        assert!(
            (srgb - back).abs() < 0.002,
            "sRGB roundtrip failed for {i}/255: {srgb} -> {linear} -> {back}"
        );
    }
}

#[test]
fn srgb_boundaries() {
    assert!((srgb_to_linear(0.0)).abs() < f32::EPSILON);
    assert!((srgb_to_linear(1.0) - 1.0).abs() < 0.001);
    assert!((linear_to_srgb(0.0)).abs() < f32::EPSILON);
    assert!((linear_to_srgb(1.0) - 1.0).abs() < 0.001);
}

#[test]
fn srgb_midpoint_is_darker_linearly() {
    // sRGB 0.5 should map to a linear value less than 0.5
    // (gamma encoding makes midtones brighter in sRGB)
    let linear = srgb_to_linear(0.5);
    assert!(linear < 0.5, "sRGB 0.5 should be < 0.5 in linear: {linear}");
    assert!(linear > 0.1, "sRGB 0.5 should be > 0.1 in linear: {linear}");
}

#[test]
fn from_srgb_u8_and_to_srgb_u8_roundtrip() {
    let color = RgbaF32::from_srgb_u8(128, 64, 200, 255);
    let bytes = color.to_srgb_u8();
    // Allow +-1 for rounding
    assert!((i16::from(bytes[0]) - 128).unsigned_abs() <= 1);
    assert!((i16::from(bytes[1]) - 64).unsigned_abs() <= 1);
    assert!((i16::from(bytes[2]) - 200).unsigned_abs() <= 1);
    assert_eq!(bytes[3], 255);
}

// ── Canvas Construction ────────────────────────────────────────────────────

#[test]
fn canvas_new_default_size() {
    let c = Canvas::default();
    assert_eq!(c.width(), DEFAULT_CANVAS_WIDTH);
    assert_eq!(c.height(), DEFAULT_CANVAS_HEIGHT);
}

#[test]
fn canvas_new_custom_size() {
    let c = Canvas::new(10, 20);
    assert_eq!(c.width(), 10);
    assert_eq!(c.height(), 20);
    assert_eq!(c.as_rgba_bytes().len(), 10 * 20 * BYTES_PER_PIXEL);
}

#[test]
fn canvas_new_filled_opaque_black() {
    let c = Canvas::new(4, 4);
    for pixel in c.pixels() {
        assert_eq!(pixel, [0, 0, 0, 255]);
    }
}

#[test]
fn canvas_from_rgba() {
    let data = vec![255, 0, 0, 255, 0, 255, 0, 255];
    let c = Canvas::from_rgba(&data, 2, 1);
    assert_eq!(c.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
    assert_eq!(c.get_pixel(1, 0), Rgba::new(0, 255, 0, 255));
}

#[test]
#[should_panic(expected = "does not match")]
fn canvas_from_rgba_wrong_size_panics() {
    let data = vec![0u8; 10];
    let _ = Canvas::from_rgba(&data, 2, 2);
}

#[test]
fn canvas_from_vec() {
    let data = vec![100, 150, 200, 255, 50, 25, 75, 128];
    let c = Canvas::from_vec(data, 2, 1);
    assert_eq!(c.get_pixel(0, 0), Rgba::new(100, 150, 200, 255));
    assert_eq!(c.get_pixel(1, 0), Rgba::new(50, 25, 75, 128));
}

#[test]
#[should_panic(expected = "does not match")]
fn canvas_from_vec_wrong_size_panics() {
    let data = vec![0u8; 5];
    let _ = Canvas::from_vec(data, 1, 1);
}

// ── Canvas Pixel Access ────────────────────────────────────────────────────

#[test]
fn canvas_get_set_pixel() {
    let mut c = Canvas::new(10, 10);
    let red = Rgba::new(255, 0, 0, 255);
    c.set_pixel(5, 5, red);
    assert_eq!(c.get_pixel(5, 5), red);
}

#[test]
fn canvas_get_pixel_oob_returns_black() {
    let c = Canvas::new(10, 10);
    assert_eq!(c.get_pixel(10, 0), Rgba::BLACK);
    assert_eq!(c.get_pixel(0, 10), Rgba::BLACK);
    assert_eq!(c.get_pixel(100, 100), Rgba::BLACK);
}

#[test]
fn canvas_set_pixel_oob_is_noop() {
    let mut c = Canvas::new(2, 2);
    let red = Rgba::new(255, 0, 0, 255);
    c.set_pixel(5, 5, red); // should not panic
    // All pixels remain opaque black
    for pixel in c.pixels() {
        assert_eq!(pixel, [0, 0, 0, 255]);
    }
}

#[test]
fn canvas_fill() {
    let mut c = Canvas::new(4, 4);
    let blue = Rgba::new(0, 0, 255, 200);
    c.fill(blue);
    for pixel in c.pixels() {
        assert_eq!(pixel, [0, 0, 255, 200]);
    }
}

#[test]
fn canvas_clear() {
    let mut c = Canvas::new(4, 4);
    c.fill(Rgba::WHITE);
    c.clear();
    for pixel in c.pixels() {
        assert_eq!(pixel, [0, 0, 0, 255]);
    }
}

#[test]
fn canvas_pixels_len() {
    let c = Canvas::new(8, 6);
    assert_eq!(c.pixels().len(), 48);
}

#[test]
fn canvas_as_rgba_bytes_mut() {
    let mut c = Canvas::new(2, 1);
    let bytes = c.as_rgba_bytes_mut();
    bytes[0] = 255; // R of first pixel
    assert_eq!(c.get_pixel(0, 0).r, 255);
}

#[test]
fn canvas_debug_format() {
    let c = Canvas::new(10, 20);
    let debug = format!("{c:?}");
    assert!(debug.contains("Canvas"));
    assert!(debug.contains("10"));
    assert!(debug.contains("20"));
}

// ── Canvas Sampling ────────────────────────────────────────────────────────

#[test]
fn sample_nearest_corners() {
    let mut c = Canvas::new(4, 4);
    let red = Rgba::new(255, 0, 0, 255);
    let green = Rgba::new(0, 255, 0, 255);
    c.set_pixel(0, 0, red);
    c.set_pixel(3, 3, green);

    assert_eq!(c.sample_nearest(0.0, 0.0), red);
    assert_eq!(c.sample_nearest(1.0, 1.0), green);
}

#[test]
fn sample_bilinear_midpoint() {
    let mut c = Canvas::new(2, 1);
    c.set_pixel(0, 0, Rgba::new(0, 0, 0, 255));
    c.set_pixel(1, 0, Rgba::new(200, 200, 200, 255));

    let mid = c.sample_bilinear(0.5, 0.0);
    // Should be approximately halfway
    assert!(mid.r > 80 && mid.r < 120, "bilinear midpoint r = {}", mid.r);
}

#[test]
fn sample_area_uniform() {
    let mut c = Canvas::new(10, 10);
    c.fill(Rgba::new(100, 100, 100, 255));

    let sampled = c.sample_area(0.5, 0.5, 2.0);
    assert_eq!(sampled, Rgba::new(100, 100, 100, 255));
}

#[test]
fn sample_dispatch() {
    let c = Canvas::new(4, 4);
    // Just verify dispatch works without panicking
    let _ = c.sample(0.5, 0.5, SamplingMethod::Nearest);
    let _ = c.sample(0.5, 0.5, SamplingMethod::Bilinear);
    let _ = c.sample(0.5, 0.5, SamplingMethod::Area { radius: 1.0 });
}

#[test]
fn sample_clamps_oob_coords() {
    let c = Canvas::new(4, 4);
    // Should not panic, coords are clamped
    let _ = c.sample(-1.0, -1.0, SamplingMethod::Nearest);
    let _ = c.sample(2.0, 2.0, SamplingMethod::Bilinear);
}

// ── BlendMode ──────────────────────────────────────────────────────────────

#[test]
fn blend_normal_full_opacity() {
    let dst = [0.2, 0.3, 0.4, 1.0];
    let src = [0.8, 0.7, 0.6, 1.0];
    let result = BlendMode::Normal.blend(dst, src, 1.0);
    // Normal at full opacity: result = src
    assert!((result[0] - 0.8).abs() < 0.01);
    assert!((result[1] - 0.7).abs() < 0.01);
    assert!((result[2] - 0.6).abs() < 0.01);
}

#[test]
fn blend_normal_zero_opacity() {
    let dst = [0.2, 0.3, 0.4, 1.0];
    let src = [0.8, 0.7, 0.6, 1.0];
    let result = BlendMode::Normal.blend(dst, src, 0.0);
    // Zero opacity: result = dst
    assert!((result[0] - 0.2).abs() < 0.01);
    assert!((result[1] - 0.3).abs() < 0.01);
    assert!((result[2] - 0.4).abs() < 0.01);
}

#[test]
fn blend_add_clamps() {
    let dst = [0.8, 0.9, 0.7, 1.0];
    let src = [0.5, 0.5, 0.5, 1.0];
    let result = BlendMode::Add.blend(dst, src, 1.0);
    // Add: clamped to 1.0
    assert!(result[0] <= 1.0);
    assert!(result[1] <= 1.0);
}

#[test]
fn blend_multiply_darkens() {
    let dst = [0.8, 0.6, 0.4, 1.0];
    let src = [0.5, 0.5, 0.5, 1.0];
    let result = BlendMode::Multiply.blend(dst, src, 1.0);
    // Multiply always darkens (result <= min(dst, src) when both < 1)
    assert!(result[0] <= 0.5);
    assert!(result[1] <= 0.5);
}

#[test]
fn blend_screen_brightens() {
    let dst = [0.3, 0.4, 0.5, 1.0];
    let src = [0.3, 0.4, 0.5, 1.0];
    let result = BlendMode::Screen.blend(dst, src, 1.0);
    // Screen always brightens
    assert!(result[0] > dst[0]);
    assert!(result[1] > dst[1]);
}

#[test]
fn blend_overlay_contrast() {
    // Overlay: multiply when dst < 0.5, screen when dst > 0.5
    let dark_dst = [0.2, 0.2, 0.2, 1.0];
    let light_dst = [0.8, 0.8, 0.8, 1.0];
    let src = [0.5, 0.5, 0.5, 1.0];

    let dark_result = BlendMode::Overlay.blend(dark_dst, src, 1.0);
    let light_result = BlendMode::Overlay.blend(light_dst, src, 1.0);

    // Dark gets darker, light gets lighter
    assert!(dark_result[0] < 0.5);
    assert!(light_result[0] > 0.5);
}

#[test]
fn blend_soft_light() {
    let dst = [0.5, 0.5, 0.5, 1.0];
    let src = [0.3, 0.7, 0.5, 1.0];
    let result = BlendMode::SoftLight.blend(dst, src, 1.0);
    // Soft light should produce values in [0, 1]
    for ch in &result[..3] {
        assert!(*ch >= 0.0 && *ch <= 1.0);
    }
}

#[test]
fn blend_color_dodge() {
    let dst = [0.4, 0.4, 0.4, 1.0];
    let src = [0.5, 0.5, 0.5, 1.0];
    let result = BlendMode::ColorDodge.blend(dst, src, 1.0);
    // Color dodge brightens: dst / (1 - src) = 0.4 / 0.5 = 0.8
    assert!((result[0] - 0.8).abs() < 0.01);
}

#[test]
fn blend_color_dodge_src_one_clamps() {
    let dst = [0.5, 0.5, 0.5, 1.0];
    let src = [1.0, 1.0, 1.0, 1.0];
    let result = BlendMode::ColorDodge.blend(dst, src, 1.0);
    // src=1.0 -> result=1.0 (clamped)
    assert!((result[0] - 1.0).abs() < 0.01);
}

#[test]
fn blend_difference() {
    let dst = [0.8, 0.3, 0.5, 1.0];
    let src = [0.3, 0.8, 0.5, 1.0];
    let result = BlendMode::Difference.blend(dst, src, 1.0);
    assert!((result[0] - 0.5).abs() < 0.01);
    assert!((result[1] - 0.5).abs() < 0.01);
    assert!((result[2]).abs() < 0.01); // |0.5 - 0.5| = 0
}

#[test]
fn blend_alpha_compositing() {
    let dst = [0.0, 0.0, 0.0, 1.0];
    let src = [1.0, 1.0, 1.0, 0.5];
    let result = BlendMode::Normal.blend(dst, src, 1.0);
    // dst_alpha + src_alpha - dst_alpha * src_alpha = 1.0 + 0.5 - 0.5 = 1.0
    assert!((result[3] - 1.0).abs() < 0.01);
}

#[test]
fn blend_mode_default_is_normal() {
    assert_eq!(BlendMode::default(), BlendMode::Normal);
}

#[test]
fn blend_mode_serde_roundtrip() {
    let modes = [
        BlendMode::Normal,
        BlendMode::Add,
        BlendMode::Screen,
        BlendMode::Multiply,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::ColorDodge,
        BlendMode::Difference,
    ];
    for mode in &modes {
        let json = serde_json::to_string(mode).expect("serialize");
        let back: BlendMode = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(*mode, back, "roundtrip failed for {json}");
    }
}

#[test]
fn blend_mode_serde_snake_case() {
    let json = serde_json::to_string(&BlendMode::SoftLight).expect("serialize");
    assert_eq!(json, "\"soft_light\"");

    let json = serde_json::to_string(&BlendMode::ColorDodge).expect("serialize");
    assert_eq!(json, "\"color_dodge\"");
}

// ── RgbaF32 blend method ──────────────────────────────────────────────────

#[test]
fn rgbaf32_blend_method() {
    let src = RgbaF32::new(1.0, 0.0, 0.0, 1.0);
    let dst = RgbaF32::new(0.0, 0.0, 1.0, 1.0);
    let result = src.blend(dst, BlendMode::Add, 1.0);
    assert!((result.r - 1.0).abs() < 0.01);
    assert!((result.b - 1.0).abs() < 0.01);
}

// ── ColorFormat ────────────────────────────────────────────────────────────

#[test]
fn color_format_default_is_rgb() {
    assert_eq!(ColorFormat::default(), ColorFormat::Rgb);
}

#[test]
fn color_format_serde() {
    let json = serde_json::to_string(&ColorFormat::RgbW16).expect("serialize");
    assert_eq!(json, "\"rgb_w16\"");
    let back: ColorFormat = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, ColorFormat::RgbW16);
}

// ── SamplingMethod ─────────────────────────────────────────────────────────

#[test]
fn sampling_method_default_is_bilinear() {
    assert_eq!(SamplingMethod::default(), SamplingMethod::Bilinear);
}

#[test]
fn sampling_method_serde() {
    let area = SamplingMethod::Area { radius: 5.0 };
    let json = serde_json::to_string(&area).expect("serialize");
    let back: SamplingMethod = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, area);
}

// ── Oklab ──────────────────────────────────────────────────────────────────

#[test]
fn oklab_roundtrip_white() {
    let white = RgbaF32::new(1.0, 1.0, 1.0, 1.0);
    let lab = white.to_oklab();
    let back = RgbaF32::from_oklab(lab);
    assert!((back.r - 1.0).abs() < 0.01, "r = {}", back.r);
    assert!((back.g - 1.0).abs() < 0.01, "g = {}", back.g);
    assert!((back.b - 1.0).abs() < 0.01, "b = {}", back.b);
}

#[test]
fn oklab_roundtrip_black() {
    let black_rgb = RgbaF32::new(0.0, 0.0, 0.0, 1.0);
    let lab = black_rgb.to_oklab();
    assert!(lab.l.abs() < 0.01, "L should be ~0 for black: {}", lab.l);
    let roundtrip = RgbaF32::from_oklab(lab);
    assert!(roundtrip.r.abs() < 0.01);
    assert!(roundtrip.g.abs() < 0.01);
    assert!(roundtrip.b.abs() < 0.01);
}

#[test]
fn oklab_roundtrip_colors() {
    let colors = [
        RgbaF32::new(1.0, 0.0, 0.0, 1.0), // red
        RgbaF32::new(0.0, 1.0, 0.0, 1.0), // green
        RgbaF32::new(0.0, 0.0, 1.0, 1.0), // blue
        RgbaF32::new(0.5, 0.3, 0.7, 0.8), // arbitrary
    ];
    for color in &colors {
        let lab = color.to_oklab();
        let back = RgbaF32::from_oklab(lab);
        assert!(
            (color.r - back.r).abs() < 0.02,
            "r mismatch: {} vs {}",
            color.r,
            back.r
        );
        assert!(
            (color.g - back.g).abs() < 0.02,
            "g mismatch: {} vs {}",
            color.g,
            back.g
        );
        assert!(
            (color.b - back.b).abs() < 0.02,
            "b mismatch: {} vs {}",
            color.b,
            back.b
        );
        assert!((color.a - back.a).abs() < f32::EPSILON, "alpha mismatch");
    }
}

#[test]
fn oklab_lerp_midpoint() {
    let black = Oklab::new(0.0, 0.0, 0.0, 1.0);
    let white = Oklab::new(1.0, 0.0, 0.0, 1.0);
    let mid = black.lerp(white, 0.5);
    assert!((mid.l - 0.5).abs() < f32::EPSILON);
}

#[test]
fn oklab_preserves_alpha() {
    let color = RgbaF32::new(0.5, 0.3, 0.7, 0.42);
    let lab = color.to_oklab();
    assert!((lab.alpha - 0.42).abs() < f32::EPSILON);
    let back = lab.to_linear_srgb();
    assert!((back.a - 0.42).abs() < f32::EPSILON);
}

#[test]
fn oklab_default() {
    let d = Oklab::default();
    assert!(d.l.abs() < f32::EPSILON);
    assert!((d.alpha - 1.0).abs() < f32::EPSILON);
}

// ── Oklch ──────────────────────────────────────────────────────────────────

#[test]
fn oklch_roundtrip_via_oklab() {
    let original = Oklch::new(0.7, 0.15, 120.0, 1.0);
    let lab = original.to_oklab();
    let back = lab.to_oklch();
    assert!((original.l - back.l).abs() < 0.001);
    assert!((original.c - back.c).abs() < 0.001);
    assert!((original.h - back.h).abs() < 0.5);
}

#[test]
fn oklch_to_linear_srgb_roundtrip() {
    let lch = Oklch::new(0.6, 0.1, 240.0, 0.9);
    let rgb = lch.to_linear_srgb();
    let back_lch = rgb.to_oklch();
    assert!((lch.l - back_lch.l).abs() < 0.02);
    assert!((lch.c - back_lch.c).abs() < 0.02);
}

#[test]
fn oklch_lerp_shortest_hue_path() {
    // 350 -> 10 should go through 0, not 180
    let a = Oklch::new(0.5, 0.1, 350.0, 1.0);
    let b = Oklch::new(0.5, 0.1, 10.0, 1.0);
    let mid = a.lerp(b, 0.5);
    // Midpoint should be near 0/360, not near 180
    assert!(
        mid.h < 10.0 || mid.h > 350.0,
        "hue should be near 0/360: {}",
        mid.h
    );
}

#[test]
fn oklch_lerp_same_direction() {
    let a = Oklch::new(0.5, 0.1, 30.0, 1.0);
    let b = Oklch::new(0.5, 0.1, 60.0, 1.0);
    let mid = a.lerp(b, 0.5);
    assert!((mid.h - 45.0).abs() < 0.1);
}

#[test]
fn oklch_default() {
    let d = Oklch::default();
    assert!(d.l.abs() < f32::EPSILON);
    assert!(d.c.abs() < f32::EPSILON);
    assert!(d.h.abs() < f32::EPSILON);
    assert!((d.alpha - 1.0).abs() < f32::EPSILON);
}

#[test]
fn oklch_serde_roundtrip() {
    let lch = Oklch::new(0.65, 0.2, 180.0, 0.95);
    let json = serde_json::to_string(&lch).expect("serialize");
    let back: Oklch = serde_json::from_str(&json).expect("deserialize");
    assert!((lch.l - back.l).abs() < f32::EPSILON);
    assert!((lch.c - back.c).abs() < f32::EPSILON);
    assert!((lch.h - back.h).abs() < f32::EPSILON);
}

// ── RgbaF32 color space convenience methods ────────────────────────────────

#[test]
fn rgbaf32_to_oklab_and_back() {
    let color = RgbaF32::new(0.8, 0.2, 0.5, 1.0);
    let lab = color.to_oklab();
    let back = RgbaF32::from_oklab(lab);
    assert!((color.r - back.r).abs() < 0.02);
    assert!((color.g - back.g).abs() < 0.02);
    assert!((color.b - back.b).abs() < 0.02);
}

#[test]
fn rgbaf32_to_oklch_and_back() {
    let color = RgbaF32::new(0.3, 0.6, 0.9, 1.0);
    let lch = color.to_oklch();
    let back = RgbaF32::from_oklch(lch);
    assert!((color.r - back.r).abs() < 0.02);
    assert!((color.g - back.g).abs() < 0.02);
    assert!((color.b - back.b).abs() < 0.02);
}

// ── Standalone conversion functions ────────────────────────────────────────

#[test]
fn linear_srgb_to_oklab_known_values() {
    // White should have L ~= 1.0, a ~= 0, b ~= 0
    let white = linear_srgb_to_oklab(1.0, 1.0, 1.0, 1.0);
    assert!((white.l - 1.0).abs() < 0.01);
    assert!(white.a.abs() < 0.01);
    assert!(white.b.abs() < 0.01);
}

#[test]
fn oklab_to_linear_srgb_known_values() {
    // L=0 should give black
    let black = oklab_to_linear_srgb(Oklab::new(0.0, 0.0, 0.0, 1.0));
    assert!(black.r.abs() < 0.01);
    assert!(black.g.abs() < 0.01);
    assert!(black.b.abs() < 0.01);
}
