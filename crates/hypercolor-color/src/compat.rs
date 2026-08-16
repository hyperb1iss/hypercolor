//! Transitional names carried over from `hypercolor-types::canvas`.
//!
//! Every item here delegates to a kernel operation defined elsewhere in
//! this crate — no new math lives in this module. The deprecated ones
//! name operations whose behavior is misleading (direct byte scaling
//! dressed as linear light) or whose canonical spelling changed
//! (`to_srgba` → `to_encoded`); they exist so the canvas absorption is a
//! type swap rather than a rewrite, and they retire one release later
//! per Spec 76 §1.2.

use crate::transfer::lut::srgb_u8_to_linear;
use crate::types::{LinearRgba, Oklab, Oklch, Rgba};

/// Linear-light RGBA under its pre-kernel name.
#[deprecated(note = "renamed to LinearRgba")]
pub type RgbaF32 = LinearRgba;

/// The canonical unit-float → byte conversion, exposed as a free
/// function for sinks that write linear-light bytes directly (LED PWM
/// after any transfer correction the device itself applies).
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "value is clamped into byte range before the cast"
)]
pub fn linear_to_output_u8(c: f32) -> u8 {
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

impl Rgba {
    /// Scale each byte channel into `0.0..=1.0` with **no** transfer
    /// decode, producing a [`LinearRgba`] that does not hold linear
    /// light.
    ///
    /// The type is a lie the canvas module told for years; it is kept
    /// only so absorbing call sites is a mechanical swap.
    #[must_use]
    #[deprecated(note = "direct-scaled bytes are not linear light; use to_linear")]
    pub fn to_f32(self) -> LinearRgba {
        LinearRgba {
            r: f32::from(self.r) / 255.0,
            g: f32::from(self.g) / 255.0,
            b: f32::from(self.b) / 255.0,
            a: f32::from(self.a) / 255.0,
        }
    }

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

    /// Scale each channel by 255 and clamp, **skipping** the sRGB
    /// transfer encode and truncating rather than rounding.
    ///
    /// Sinks that consume linear-light bytes used this; it is not the
    /// inverse of [`Rgba::to_linear`] and never was.
    #[must_use]
    #[deprecated(note = "direct-scaled bytes are not encoded sRGB; use to_encoded")]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions,
        reason = "channels are clamped into byte range before the cast"
    )]
    pub fn to_rgba(self) -> Rgba {
        Rgba {
            r: (self.r * 255.0).clamp(0.0, 255.0) as u8,
            g: (self.g * 255.0).clamp(0.0, 255.0) as u8,
            b: (self.b * 255.0).clamp(0.0, 255.0) as u8,
            a: (self.a * 255.0).clamp(0.0, 255.0) as u8,
        }
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
