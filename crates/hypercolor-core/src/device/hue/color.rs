//! RGB → CIE xy conversion utilities for Philips Hue.

use serde::{Deserialize, Serialize};

/// CIE 1931 xy chromaticity + brightness.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CieXyb {
    pub x: f64,
    pub y: f64,
    pub brightness: f64,
}

/// Hue device color gamut triangle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorGamut {
    pub red: (f64, f64),
    pub green: (f64, f64),
    pub blue: (f64, f64),
}

/// Hue gamut used by classic bulbs.
pub const GAMUT_A: ColorGamut = ColorGamut {
    red: (0.704, 0.296),
    green: (0.2151, 0.7106),
    blue: (0.138, 0.08),
};

/// Hue gamut used by many mid-generation lights.
pub const GAMUT_B: ColorGamut = ColorGamut {
    red: (0.675, 0.322),
    green: (0.409, 0.518),
    blue: (0.167, 0.04),
};

/// Hue gamut used by modern entertainment-capable lights.
pub const GAMUT_C: ColorGamut = ColorGamut {
    red: (0.6915, 0.3083),
    green: (0.17, 0.7),
    blue: (0.1532, 0.0475),
};

const D65_WHITE_POINT: (f64, f64) = (0.3127, 0.3290);

/// Convert sRGB (0-255) to CIE xy + brightness.
#[must_use]
#[expect(
    clippy::many_single_char_names,
    reason = "CIE xy/XYZ notation uses standard single-letter color-science variables"
)]
pub fn rgb_to_cie_xyb(r: u8, g: u8, b: u8, gamut: &ColorGamut) -> CieXyb {
    let red = linearize_channel(r);
    let green = linearize_channel(g);
    let blue = linearize_channel(b);

    let x = red * 0.664_511 + green * 0.154_324 + blue * 0.162_028;
    let y = red * 0.283_881 + green * 0.668_433 + blue * 0.047_685;
    let z = red * 0.000_088 + green * 0.072_31 + blue * 0.986_039;
    let brightness = y.clamp(0.0, 1.0);

    let (x, y) = xyz_to_xy(x, y, z);
    let (x, y) = if point_in_gamut(x, y, gamut) {
        (x, y)
    } else {
        clamp_to_gamut(x, y, gamut)
    };

    CieXyb { x, y, brightness }
}

fn linearize_channel(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value > 0.04045 {
        ((value + 0.055) / 1.055).powf(2.4)
    } else {
        value / 12.92
    }
}

fn xyz_to_xy(x: f64, y: f64, z: f64) -> (f64, f64) {
    let sum = x + y + z;
    if sum <= f64::EPSILON {
        return D65_WHITE_POINT;
    }

    (x / sum, y / sum)
}

fn point_in_gamut(x: f64, y: f64, gamut: &ColorGamut) -> bool {
    let point = (x, y);
    let v1 = gamut.red;
    let v2 = gamut.green;
    let v3 = gamut.blue;

    let d1 = sign(point, v1, v2);
    let d2 = sign(point, v2, v3);
    let d3 = sign(point, v3, v1);

    let has_negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;

    !(has_negative && has_positive)
}

fn clamp_to_gamut(x: f64, y: f64, gamut: &ColorGamut) -> (f64, f64) {
    let point = (x, y);
    let edges = [
        (gamut.red, gamut.green),
        (gamut.green, gamut.blue),
        (gamut.blue, gamut.red),
    ];

    let mut best_point = gamut.red;
    let mut best_distance = squared_distance(point, best_point);

    for (start, end) in edges {
        let candidate = closest_point_on_segment(point, start, end);
        let distance = squared_distance(point, candidate);
        if distance < best_distance {
            best_distance = distance;
            best_point = candidate;
        }
    }

    best_point
}

fn closest_point_on_segment(point: (f64, f64), start: (f64, f64), end: (f64, f64)) -> (f64, f64) {
    let segment = (end.0 - start.0, end.1 - start.1);
    let length_squared = segment.0.mul_add(segment.0, segment.1 * segment.1);
    if length_squared <= f64::EPSILON {
        return start;
    }

    let projection =
        ((point.0 - start.0) * segment.0 + (point.1 - start.1) * segment.1) / length_squared;
    let t = projection.clamp(0.0, 1.0);

    (start.0 + segment.0 * t, start.1 + segment.1 * t)
}

fn sign(point: (f64, f64), left: (f64, f64), right: (f64, f64)) -> f64 {
    (point.0 - right.0).mul_add(left.1 - right.1, -(left.0 - right.0) * (point.1 - right.1))
}

fn squared_distance(left: (f64, f64), right: (f64, f64)) -> f64 {
    let dx = left.0 - right.0;
    let dy = left.1 - right.1;
    dx.mul_add(dx, dy * dy)
}
