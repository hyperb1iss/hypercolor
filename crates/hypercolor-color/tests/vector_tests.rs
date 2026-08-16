//! Shared cross-language vector tests (Spec 76 §1.6).
//!
//! `sdk/shared/color-vectors.json` is the drift fence between the Rust
//! kernel, the TS SDK color module, and any future implementation: the
//! same vectors run in every language, so divergence fails CI on both
//! sides instead of shipping as a subtle rendering mismatch.

use hypercolor_color::{Hsl, Hsv, LinearRgba, Rgb, Rgba, linear_to_srgb, srgb_to_linear};
use serde_json::Value;

const VECTORS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../sdk/shared/color-vectors.json"
));

fn as_f32(v: &Value) -> f32 {
    let f = v.as_f64().expect("vector number");
    f as f32
}

fn as_u8(v: &Value) -> u8 {
    u8::try_from(v.as_u64().expect("vector byte")).expect("vector byte range")
}

fn u8_triple(v: &Value) -> [u8; 3] {
    let arr = v.as_array().expect("vector byte triple");
    [as_u8(&arr[0]), as_u8(&arr[1]), as_u8(&arr[2])]
}

fn u8_quad(v: &Value) -> [u8; 4] {
    let arr = v.as_array().expect("vector byte quad");
    [
        as_u8(&arr[0]),
        as_u8(&arr[1]),
        as_u8(&arr[2]),
        as_u8(&arr[3]),
    ]
}

fn f32_list(v: &Value) -> Vec<f32> {
    v.as_array()
        .expect("vector float list")
        .iter()
        .map(as_f32)
        .collect()
}

fn assert_scalar(actual: f32, expected: f32, tol: f32, ctx: &str) {
    assert!(
        (actual - expected).abs() <= tol,
        "{ctx}: actual={actual}, expected={expected}, tol={tol}"
    );
}

fn assert_hue(actual: f32, expected: f32, tol: f32, ctx: &str) {
    let diff = (actual - expected).abs().rem_euclid(360.0);
    let angular = diff.min(360.0 - diff);
    assert!(
        angular <= tol,
        "{ctx}: actual hue={actual}, expected={expected}, tol={tol}"
    );
}

#[test]
fn shared_vectors_all_pass() {
    let doc: Value = serde_json::from_str(VECTORS).expect("color-vectors.json parses");
    let default_tol = as_f32(&doc["default_tolerance"]);
    let vectors = doc["vectors"].as_array().expect("vectors array");
    assert!(!vectors.is_empty(), "vector file must not be empty");

    for (index, vector) in vectors.iter().enumerate() {
        let op = vector["op"].as_str().expect("vector op");
        let input = &vector["input"];
        let expected = &vector["expected"];
        let tol = vector.get("tol").map_or(default_tol, as_f32);
        let ctx = format!("vector #{index} ({op}, input {input})");

        match op {
            "hex_to_rgb" => {
                let parsed = Rgb::from_hex(input.as_str().expect("hex input"));
                match (parsed, expected.is_null()) {
                    (Err(_), true) => {}
                    (Ok(rgb), false) => {
                        assert_eq!([rgb.r, rgb.g, rgb.b], u8_triple(expected), "{ctx}");
                    }
                    (Ok(rgb), true) => panic!("{ctx}: expected parse failure, got {rgb:?}"),
                    (Err(err), false) => panic!("{ctx}: expected success, got {err}"),
                }
            }
            "hex_to_rgba" => {
                let parsed = Rgba::from_hex(input.as_str().expect("hex input"));
                match (parsed, expected.is_null()) {
                    (Err(_), true) => {}
                    (Ok(c), false) => {
                        assert_eq!([c.r, c.g, c.b, c.a], u8_quad(expected), "{ctx}");
                    }
                    (Ok(c), true) => panic!("{ctx}: expected parse failure, got {c:?}"),
                    (Err(err), false) => panic!("{ctx}: expected success, got {err}"),
                }
            }
            "rgb_to_hex" => {
                let [r, g, b] = u8_triple(input);
                assert_eq!(
                    Rgb::new(r, g, b).to_hex(),
                    expected.as_str().expect("hex expected"),
                    "{ctx}"
                );
            }
            "hsv_to_rgb" => {
                let i = f32_list(input);
                let rgb = Hsv::new(i[0], i[1], i[2]).to_rgb();
                assert_eq!([rgb.r, rgb.g, rgb.b], u8_triple(expected), "{ctx}");
            }
            "hsl_to_rgb" => {
                let i = f32_list(input);
                let rgb = Hsl::new(i[0], i[1], i[2]).to_rgb();
                assert_eq!([rgb.r, rgb.g, rgb.b], u8_triple(expected), "{ctx}");
            }
            "rgb_to_hsv" => {
                let [r, g, b] = u8_triple(input);
                let e = f32_list(expected);
                let hsv = Hsv::from_rgb(Rgb::new(r, g, b));
                assert_hue(hsv.h, e[0], tol, &ctx);
                assert_scalar(hsv.s, e[1], tol, &ctx);
                assert_scalar(hsv.v, e[2], tol, &ctx);
            }
            "rgb_to_hsl" => {
                let [r, g, b] = u8_triple(input);
                let e = f32_list(expected);
                let hsl = Hsl::from_rgb(Rgb::new(r, g, b));
                assert_hue(hsl.h, e[0], tol, &ctx);
                assert_scalar(hsl.s, e[1], tol, &ctx);
                assert_scalar(hsl.l, e[2], tol, &ctx);
            }
            "srgb_to_linear" => {
                assert_scalar(srgb_to_linear(as_f32(input)), as_f32(expected), tol, &ctx);
            }
            "linear_to_srgb" => {
                assert_scalar(linear_to_srgb(as_f32(input)), as_f32(expected), tol, &ctx);
            }
            "oklab_from_linear" => {
                let i = f32_list(input);
                let e = f32_list(expected);
                let lab = LinearRgba::new(i[0], i[1], i[2], i[3]).to_oklab();
                assert_scalar(lab.l, e[0], tol, &ctx);
                assert_scalar(lab.a, e[1], tol, &ctx);
                assert_scalar(lab.b, e[2], tol, &ctx);
                assert_scalar(lab.alpha, e[3], tol, &ctx);
            }
            "oklab_roundtrip" => {
                let i = f32_list(input);
                let e = f32_list(expected);
                let back = LinearRgba::new(i[0], i[1], i[2], i[3])
                    .to_oklab()
                    .to_linear();
                assert_scalar(back.r, e[0], tol, &ctx);
                assert_scalar(back.g, e[1], tol, &ctx);
                assert_scalar(back.b, e[2], tol, &ctx);
                assert_scalar(back.a, e[3], tol, &ctx);
            }
            "scale_rgb" => {
                let args = input.as_array().expect("scale input");
                let [r, g, b] = u8_triple(&args[0]);
                let factor = as_f32(&args[1]);
                let scaled = Rgb::new(r, g, b).scale(factor);
                assert_eq!([scaled.r, scaled.g, scaled.b], u8_triple(expected), "{ctx}");
            }
            other => panic!("{ctx}: unknown op {other} — add a handler before adding vectors"),
        }
    }
}
