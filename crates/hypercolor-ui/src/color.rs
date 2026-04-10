//! Shared color utilities — palette extraction and harmonization.
//!
//! The canvas frame pixels we get from the daemon represent whatever the
//! effect is rendering, which can be anywhere from deep maroons to neon
//! whites. The functions here extract a dominant 3-color palette from a
//! canvas frame and normalize it into a readable, cohesive band suitable
//! for UI accents and text coloring on dark surfaces.

use crate::ws::CanvasFrame;

/// Three-color palette extracted from a canvas frame.
#[derive(Clone, Copy, Debug)]
pub struct CanvasPalette {
    pub primary: (f64, f64, f64),
    pub secondary: (f64, f64, f64),
    pub tertiary: (f64, f64, f64),
}

/// Linear interpolate between two RGB values.
pub fn lerp_rgb(a: (f64, f64, f64), b: (f64, f64, f64), t: f64) -> (f64, f64, f64) {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

/// Linear interpolate between two palettes (for smooth color transitions).
pub fn lerp_palette(a: CanvasPalette, b: CanvasPalette, t: f64) -> CanvasPalette {
    CanvasPalette {
        primary: lerp_rgb(a.primary, b.primary, t),
        secondary: lerp_rgb(a.secondary, b.secondary, t),
        tertiary: lerp_rgb(a.tertiary, b.tertiary, t),
    }
}

/// Format RGB as "r, g, b" for CSS `rgb(...)` / `rgba(...)` interpolation.
pub fn rgb_string(c: (f64, f64, f64)) -> String {
    format!("{:.0}, {:.0}, {:.0}", c.0, c.1, c.2)
}

/// Convert RGB (0-255) to HSL (h: 0-360, s/l: 0-1).
pub fn rgb_to_hsl(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let rf = r / 255.0;
    let gf = g / 255.0;
    let bf = b / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let l = (max + min) / 2.0;

    let d = max - min;
    if d < f64::EPSILON {
        return (0.0, 0.0, l);
    }

    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };

    let h = if (max - rf).abs() < f64::EPSILON {
        60.0 * ((gf - bf) / d).rem_euclid(6.0)
    } else if (max - gf).abs() < f64::EPSILON {
        60.0 * (((bf - rf) / d) + 2.0)
    } else {
        60.0 * (((rf - gf) / d) + 4.0)
    };

    (h, s, l)
}

/// Convert HSL (h: 0-360, s/l: 0-1) to RGB (0-255).
pub fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    if s < f64::EPSILON {
        let v = l * 255.0;
        return (v, v, v);
    }

    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = (h / 60.0).rem_euclid(6.0);
    let x = c * (1.0 - (h_prime.rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;

    let (r1, g1, b1) = if h_prime < 1.0 {
        (c, x, 0.0)
    } else if h_prime < 2.0 {
        (x, c, 0.0)
    } else if h_prime < 3.0 {
        (0.0, c, x)
    } else if h_prime < 4.0 {
        (0.0, x, c)
    } else if h_prime < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    ((r1 + m) * 255.0, (g1 + m) * 255.0, (b1 + m) * 255.0)
}

/// Normalize a palette color into a readable, cohesive band.
///
/// Locks lightness to a fixed target so all palette colors sit in the same
/// visual register (looks coordinated instead of one bright + one black),
/// and clamps saturation to avoid both neon-scream and washed-out-mud.
/// Preserves hue so the effect's color identity carries through.
pub fn harmonize_rgb(c: (f64, f64, f64)) -> (f64, f64, f64) {
    let (h, s, _) = rgb_to_hsl(c.0, c.1, c.2);
    let target_l = 0.72;
    let target_s = (s * 0.5 + 0.4).clamp(0.55, 0.85);
    hsl_to_rgb(h, target_s, target_l)
}

/// Apply `harmonize_rgb` to every color in a palette.
pub fn harmonize_palette(p: CanvasPalette) -> CanvasPalette {
    CanvasPalette {
        primary: harmonize_rgb(p.primary),
        secondary: harmonize_rgb(p.secondary),
        tertiary: harmonize_rgb(p.tertiary),
    }
}

/// Extract the 1-3 most dominant vibrant colors from RGBA pixel data.
///
/// Samples ~200 pixels, groups by 12 hue sectors of 30° each, skips
/// dark/desaturated pixels, and returns averaged RGB for the top 3 sectors.
/// Returns `None` if no sector has enough vibrant pixels.
pub fn extract_canvas_palette(frame: &CanvasFrame) -> Option<CanvasPalette> {
    let pixel_count = frame.pixel_count();
    if pixel_count < 4 {
        return None;
    }

    let step = (pixel_count / 200).max(1);

    // 12 hue sectors (30° each): (r_sum, g_sum, b_sum, count)
    let mut sectors = [(0.0_f64, 0.0_f64, 0.0_f64, 0_u32); 12];

    for i in (0..pixel_count).step_by(step) {
        let Some([r, g, b, _]) = frame.rgba_at(i) else {
            continue;
        };
        let r = f64::from(r);
        let g = f64::from(g);
        let b = f64::from(b);

        let rf = r / 255.0;
        let gf = g / 255.0;
        let bf = b / 255.0;

        let max = rf.max(gf).max(bf);
        let min = rf.min(gf).min(bf);
        let chroma = max - min;
        let lightness = (max + min) / 2.0;

        if chroma < 0.15 || lightness < 0.08 {
            continue;
        }

        let hue = if (max - rf).abs() < f64::EPSILON {
            60.0 * (((gf - bf) / chroma) % 6.0)
        } else if (max - gf).abs() < f64::EPSILON {
            60.0 * (((bf - rf) / chroma) + 2.0)
        } else {
            60.0 * (((rf - gf) / chroma) + 4.0)
        };
        let hue = if hue < 0.0 { hue + 360.0 } else { hue };

        let sector = ((hue / 30.0) as usize).min(11);
        sectors[sector].0 += r;
        sectors[sector].1 += g;
        sectors[sector].2 += b;
        sectors[sector].3 += 1;
    }

    let mut ranked: Vec<(usize, u32)> = sectors
        .iter()
        .enumerate()
        .filter(|(_, s)| s.3 >= 3)
        .map(|(i, s)| (i, s.3))
        .collect();

    if ranked.is_empty() {
        return None;
    }

    ranked.sort_by(|a, b| b.1.cmp(&a.1));

    let avg = |idx: usize| -> (f64, f64, f64) {
        let s = &sectors[idx];
        let n = f64::from(s.3);
        (s.0 / n, s.1 / n, s.2 / n)
    };

    let primary = avg(ranked[0].0);
    let secondary = if ranked.len() > 1 {
        avg(ranked[1].0)
    } else {
        primary
    };
    let tertiary = if ranked.len() > 2 {
        avg(ranked[2].0)
    } else {
        secondary
    };

    Some(CanvasPalette {
        primary,
        secondary,
        tertiary,
    })
}
