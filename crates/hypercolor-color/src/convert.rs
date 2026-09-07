//! Color-space conversions: HSV/HSL ↔ RGB, byte ↔ linear, luminance,
//! interpolation, and the Oklab/Oklch transforms.

// r/g/b/h/s/v/l ARE the domain vocabulary here.
#![allow(clippy::many_single_char_names)]

use crate::transfer::lut::{linear_to_srgb_u8, srgb_u8_to_linear};
use crate::types::{Hsl, Hsv, LinearRgba, Oklab, Oklch, Rgb, Rgba};
use crate::{LUMA_B, LUMA_G, LUMA_R, wrap_hue};

/// The one canonical unit-float → byte conversion: round, then clamp.
#[inline]
fn unit_to_u8(x: f32) -> u8 {
    (x * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The canonical unit-float → byte conversion, exposed as a free
/// function for sinks that write linear-light bytes directly (LED PWM
/// after any transfer correction the device itself applies).
#[inline]
#[must_use]
pub fn linear_to_output_u8(c: f32) -> u8 {
    unit_to_u8(c)
}

#[inline]
fn rgb_from_units(r: f32, g: f32, b: f32) -> Rgb {
    Rgb {
        r: unit_to_u8(r),
        g: unit_to_u8(g),
        b: unit_to_u8(b),
    }
}

/// Shared sector decomposition for the chroma-based HSV/HSL algorithms:
/// hue in `[0, 360)` with chroma `c` and intermediate `x` produce the
/// unit RGB triple before the match-lightness offset is added.
#[inline]
fn sector_rgb(h: f32, c: f32) -> (f32, f32, f32) {
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    }
}

impl Hsv {
    /// Convert to encoded RGB bytes.
    ///
    /// Hue wraps into `[0, 360)`; saturation and value clamp to `[0, 1]`.
    #[must_use]
    pub fn to_rgb(self) -> Rgb {
        let h = wrap_hue(self.h);
        let s = self.s.clamp(0.0, 1.0);
        let v = self.v.clamp(0.0, 1.0);
        let c = v * s;
        let (r1, g1, b1) = sector_rgb(h, c);
        let m = v - c;
        rgb_from_units(r1 + m, g1 + m, b1 + m)
    }

    /// Convert encoded RGB bytes to HSV.
    #[must_use]
    #[allow(clippy::float_cmp)] // comparing against a max we just computed from the same operands
    pub fn from_rgb(rgb: Rgb) -> Hsv {
        let r = f32::from(rgb.r) / 255.0;
        let g = f32::from(rgb.g) / 255.0;
        let b = f32::from(rgb.b) / 255.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let h = hue_from_deltas(r, g, b, max, delta);
        let s = if max == 0.0 { 0.0 } else { delta / max };
        Hsv { h, s, v: max }
    }
}

impl Hsl {
    /// Convert to encoded RGB bytes.
    ///
    /// Hue wraps into `[0, 360)`; saturation and lightness clamp to `[0, 1]`.
    #[must_use]
    pub fn to_rgb(self) -> Rgb {
        let h = wrap_hue(self.h);
        let s = self.s.clamp(0.0, 1.0);
        let l = self.l.clamp(0.0, 1.0);
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let (r1, g1, b1) = sector_rgb(h, c);
        let m = l - c / 2.0;
        rgb_from_units(r1 + m, g1 + m, b1 + m)
    }

    /// Convert encoded RGB bytes to HSL.
    #[must_use]
    #[allow(clippy::float_cmp)] // comparing against a max we just computed from the same operands
    pub fn from_rgb(rgb: Rgb) -> Hsl {
        let r = f32::from(rgb.r) / 255.0;
        let g = f32::from(rgb.g) / 255.0;
        let b = f32::from(rgb.b) / 255.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let h = hue_from_deltas(r, g, b, max, delta);
        let l = f32::midpoint(max, min);
        let s = if delta == 0.0 {
            0.0
        } else {
            delta / (1.0 - (2.0 * l - 1.0).abs())
        };
        Hsl { h, s, l }
    }
}

#[inline]
#[allow(clippy::float_cmp)] // comparing against a max we just computed from the same operands
fn hue_from_deltas(r: f32, g: f32, b: f32, max: f32, delta: f32) -> f32 {
    if delta == 0.0 {
        return 0.0;
    }
    let h = if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    wrap_hue(h)
}

impl Rgb {
    /// Decode sRGB bytes into linear light, alpha = 1.0.
    #[must_use]
    pub fn to_linear(self) -> LinearRgba {
        LinearRgba {
            r: srgb_u8_to_linear(self.r),
            g: srgb_u8_to_linear(self.g),
            b: srgb_u8_to_linear(self.b),
            a: 1.0,
        }
    }

    /// BT.709 luma computed directly on the encoded bytes.
    ///
    /// This is gamma-incorrect on purpose — it matches the screen-capture
    /// fast paths that weight encoded values. Use [`LinearRgba::luma`]
    /// for physically meaningful luminance.
    #[must_use]
    pub fn luma_encoded(self) -> f32 {
        LUMA_R.mul_add(
            f32::from(self.r) / 255.0,
            LUMA_G.mul_add(
                f32::from(self.g) / 255.0,
                LUMA_B * (f32::from(self.b) / 255.0),
            ),
        )
    }
}

impl Rgba {
    /// Decode sRGB bytes into linear light; alpha maps linearly
    /// (alpha is never gamma encoded).
    #[must_use]
    pub fn to_linear(self) -> LinearRgba {
        LinearRgba {
            r: srgb_u8_to_linear(self.r),
            g: srgb_u8_to_linear(self.g),
            b: srgb_u8_to_linear(self.b),
            a: f32::from(self.a) / 255.0,
        }
    }
}

impl LinearRgba {
    /// Gamma-encode back to sRGB bytes; alpha maps linearly with the
    /// canonical round-clamp.
    #[must_use]
    pub fn to_encoded(self) -> Rgba {
        Rgba {
            r: linear_to_srgb_u8(self.r),
            g: linear_to_srgb_u8(self.g),
            b: linear_to_srgb_u8(self.b),
            a: unit_to_u8(self.a),
        }
    }

    /// BT.709 relative luminance on linear values — the default
    /// luminance for all perceptual decisions.
    #[must_use]
    pub fn luma(self) -> f32 {
        LUMA_R.mul_add(self.r, LUMA_G.mul_add(self.g, LUMA_B * self.b))
    }

    /// Linear interpolation, componentwise including alpha.
    ///
    /// `t = 0.0` returns `self`, `t = 1.0` returns `other`.
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            r: (other.r - self.r).mul_add(t, self.r),
            g: (other.g - self.g).mul_add(t, self.g),
            b: (other.b - self.b).mul_add(t, self.b),
            a: (other.a - self.a).mul_add(t, self.a),
        }
    }

    /// Convert to Oklab. Alpha is carried through unchanged.
    #[must_use]
    pub fn to_oklab(self) -> Oklab {
        // Oklab forward transform (Björn Ottosson, 2020): linear sRGB → LMS
        // via M1, cube-root compression, LMS → Lab via M2.
        let lms_l = 0.412_221_46_f32.mul_add(
            self.r,
            0.536_332_55_f32.mul_add(self.g, 0.051_445_99 * self.b),
        );
        let lms_m = 0.211_903_5_f32.mul_add(
            self.r,
            0.680_699_5_f32.mul_add(self.g, 0.107_396_96 * self.b),
        );
        let lms_s = 0.088_302_46_f32.mul_add(
            self.r,
            0.281_718_84_f32.mul_add(self.g, 0.629_978_7 * self.b),
        );

        let l_ = lms_l.cbrt();
        let m_ = lms_m.cbrt();
        let s_ = lms_s.cbrt();

        Oklab {
            l: 0.210_454_26_f32.mul_add(l_, 0.793_617_8_f32.mul_add(m_, -0.004_072_047 * s_)),
            a: 1.977_998_5_f32.mul_add(l_, (-2.428_592_2_f32).mul_add(m_, 0.450_593_7 * s_)),
            b: 0.025_904_037_f32.mul_add(l_, 0.782_771_8_f32.mul_add(m_, -0.808_675_77 * s_)),
            alpha: self.a,
        }
    }
}

impl Oklab {
    /// Convert back to linear sRGB. Out-of-gamut inputs produce channel
    /// values outside `[0, 1]` — clamp at the byte boundary, not here.
    /// Alpha is carried through unchanged.
    #[must_use]
    pub fn to_linear(self) -> LinearRgba {
        let lms_l = self
            .l
            .mul_add(1.0, 0.396_337_78_f32.mul_add(self.a, 0.215_803_76 * self.b));
        let lms_m = self.l.mul_add(
            1.0,
            (-0.105_561_346_f32).mul_add(self.a, -0.063_854_17 * self.b),
        );
        let lms_s = self.l.mul_add(
            1.0,
            (-0.089_484_18_f32).mul_add(self.a, -1.291_485_5 * self.b),
        );

        let lin_l = lms_l * lms_l * lms_l;
        let lin_m = lms_m * lms_m * lms_m;
        let lin_s = lms_s * lms_s * lms_s;

        LinearRgba {
            r: 4.076_741_7_f32.mul_add(
                lin_l,
                (-3.307_711_6_f32).mul_add(lin_m, 0.230_969_94 * lin_s),
            ),
            g: (-1.268_438_f32)
                .mul_add(lin_l, 2.609_757_4_f32.mul_add(lin_m, -0.341_319_38 * lin_s)),
            b: (-0.004_196_086_3_f32).mul_add(
                lin_l,
                (-0.703_418_6_f32).mul_add(lin_m, 1.707_614_7 * lin_s),
            ),
            a: self.alpha,
        }
    }

    /// Linear interpolation in Oklab space (perceptually smooth),
    /// componentwise including alpha.
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            l: (other.l - self.l).mul_add(t, self.l),
            a: (other.a - self.a).mul_add(t, self.a),
            b: (other.b - self.b).mul_add(t, self.b),
            alpha: (other.alpha - self.alpha).mul_add(t, self.alpha),
        }
    }
}

impl Oklch {
    /// Convert to the cartesian [`Oklab`] form. Hue wraps at entry, so
    /// `h = 360.0` produces bit-identical output to `h = 0.0`.
    #[must_use]
    pub fn to_oklab(self) -> Oklab {
        let h_rad = wrap_hue(self.h).to_radians();
        Oklab {
            l: self.l,
            a: self.c * h_rad.cos(),
            b: self.c * h_rad.sin(),
            alpha: self.alpha,
        }
    }

    /// Convert from the cartesian [`Oklab`] form. Hue lands in `[0, 360)`.
    #[must_use]
    pub fn from_oklab(lab: Oklab) -> Oklch {
        let c = lab.a.hypot(lab.b);
        let h = wrap_hue(lab.b.atan2(lab.a).to_degrees());
        Oklch {
            l: lab.l,
            c,
            h,
            alpha: lab.alpha,
        }
    }

    /// Convert straight to linear sRGB (through [`Oklab`]).
    #[must_use]
    pub fn to_linear(self) -> LinearRgba {
        self.to_oklab().to_linear()
    }

    /// Interpolate in Oklch space with shortest-path hue interpolation:
    /// the hue takes the shortest arc around the wheel, so a
    /// `350° → 10°` gradient passes through red, not through cyan.
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        let mut dh = wrap_hue(other.h) - wrap_hue(self.h);
        if dh > 180.0 {
            dh -= 360.0;
        } else if dh < -180.0 {
            dh += 360.0;
        }
        Self {
            l: (other.l - self.l).mul_add(t, self.l),
            c: (other.c - self.c).mul_add(t, self.c),
            h: wrap_hue(dh.mul_add(t, wrap_hue(self.h))),
            alpha: (other.alpha - self.alpha).mul_add(t, self.alpha),
        }
    }
}

impl LinearRgba {
    /// Convert straight to Oklch (through [`Oklab`]).
    #[must_use]
    pub fn to_oklch(self) -> Oklch {
        Oklch::from_oklab(self.to_oklab())
    }
}
