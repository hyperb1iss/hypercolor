//! Transitional names carried over from `hypercolor-types::canvas`.
//!
//! Everything here delegates to a kernel operation defined elsewhere in
//! this crate, either under a spelling that changed (`to_srgba` →
//! `to_encoded`) or as a convenience over one that did not. None of it
//! is new math.
//!
//! These names exist so the canvas absorption is a type swap rather
//! than a rewrite, and they retire one release later per Spec 76 §1.2.

use crate::transfer::lut::srgb_u8_to_linear;
use crate::types::{LinearRgba, Oklab, Oklch, Rgba};

/// Linear-light RGBA under its pre-kernel name.
#[deprecated(note = "renamed to LinearRgba")]
pub type RgbaF32 = LinearRgba;

impl Rgba {
    /// Decode sRGB bytes into linear light.
    #[must_use]
    #[deprecated(note = "renamed to to_linear")]
    pub fn to_linear_f32(self) -> LinearRgba {
        self.to_linear()
    }
}

impl LinearRgba {
    /// Build from sRGB bytes, decoding the transfer function.
    #[must_use]
    pub fn from_srgb_u8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: srgb_u8_to_linear(r),
            g: srgb_u8_to_linear(g),
            b: srgb_u8_to_linear(b),
            a: f32::from(a) / 255.0,
        }
    }

    /// Gamma-encode back to sRGB bytes as a plain array.
    #[must_use]
    pub fn to_srgb_u8(self) -> [u8; 4] {
        let encoded = self.to_encoded();
        [encoded.r, encoded.g, encoded.b, encoded.a]
    }

    /// Gamma-encode back to byte [`Rgba`].
    #[must_use]
    #[deprecated(note = "renamed to to_encoded")]
    pub fn to_srgba(self) -> Rgba {
        self.to_encoded()
    }
}

impl Oklab {
    /// Convert back to linear sRGB.
    #[must_use]
    #[deprecated(note = "renamed to to_linear")]
    pub fn to_linear_srgb(self) -> LinearRgba {
        self.to_linear()
    }
}

impl Oklch {
    /// Convert to linear sRGB through [`Oklab`].
    #[must_use]
    #[deprecated(note = "renamed to to_linear")]
    pub fn to_linear_srgb(self) -> LinearRgba {
        self.to_linear()
    }
}

/// Convert linear sRGB channels to [`Oklab`].
#[must_use]
#[deprecated(note = "use LinearRgba::to_oklab")]
pub fn linear_srgb_to_oklab(r: f32, g: f32, b: f32, alpha: f32) -> Oklab {
    LinearRgba { r, g, b, a: alpha }.to_oklab()
}

/// Convert [`Oklab`] back to linear sRGB.
#[must_use]
#[deprecated(note = "use Oklab::to_linear")]
pub fn oklab_to_linear_srgb(lab: Oklab) -> LinearRgba {
    lab.to_linear()
}
