#![allow(dead_code, unused_imports)]

#[path = "../src/api/mod.rs"]
mod api;

#[path = "../src/ws/mod.rs"]
mod ws;

#[path = "../src/color.rs"]
mod color;

use color::{analyze_rgba_samples, extract_canvas_palette};

#[test]
fn analyze_rgba_samples_extracts_palette_and_dominant_hue_in_one_pass() {
    let samples = [
        [255, 32, 32, 255],
        [250, 20, 20, 255],
        [245, 30, 30, 255],
        [240, 25, 25, 255],
        [235, 40, 40, 255],
        [32, 255, 128, 255],
        [28, 250, 124, 255],
        [36, 245, 132, 255],
    ];

    let analysis = analyze_rgba_samples(samples).expect("expected chromatic analysis");

    assert!(analysis.palette.primary.0 > analysis.palette.primary.1);
    assert!(analysis.palette.primary.0 > analysis.palette.primary.2);
    assert!(analysis.dominant_hue <= 360.0);
    assert!(analysis.dominant_hue >= 0.0);
}

#[test]
fn analyze_rgba_samples_returns_none_for_grayscale_samples() {
    let samples = [
        [24, 24, 24, 255],
        [64, 64, 64, 255],
        [120, 120, 120, 255],
        [200, 200, 200, 255],
        [240, 240, 240, 255],
    ];

    assert!(analyze_rgba_samples(samples).is_none());
}
