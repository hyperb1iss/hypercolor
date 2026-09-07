//! Pixel and color-space value types.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Encoded sRGB color, no alpha. This is what device backends receive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Rgb {
    /// Red channel (0–255, sRGB encoded).
    pub r: u8,
    /// Green channel (0–255, sRGB encoded).
    pub g: u8,
    /// Blue channel (0–255, sRGB encoded).
    pub b: u8,
}

impl Rgb {
    /// Opaque black.
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0 };

    /// Opaque white.
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
    };

    /// Create an RGB color from individual channel values.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Promote to [`Rgba`] with full opacity.
    #[must_use]
    pub const fn to_rgba(self) -> Rgba {
        Rgba {
            r: self.r,
            g: self.g,
            b: self.b,
            a: 255,
        }
    }
}

/// Encoded sRGB color with straight (non-premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Rgba {
    /// Red channel (0–255, sRGB encoded).
    pub r: u8,
    /// Green channel (0–255, sRGB encoded).
    pub g: u8,
    /// Blue channel (0–255, sRGB encoded).
    pub b: u8,
    /// Alpha channel (0 = transparent, 255 = opaque).
    pub a: u8,
}

impl Rgba {
    /// Opaque black.
    pub const BLACK: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };

    /// Opaque white.
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };

    /// Fully transparent black.
    pub const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    /// Create an RGBA pixel from individual channel values.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Drop alpha, keeping the encoded RGB channels.
    #[must_use]
    pub const fn to_rgb(self) -> Rgb {
        Rgb {
            r: self.r,
            g: self.g,
            b: self.b,
        }
    }
}

impl Default for Rgba {
    fn default() -> Self {
        Self::BLACK
    }
}

/// Linear-light RGBA color with straight alpha, `0.0..=1.0` per channel.
///
/// All interpolation, blending, and perceptual conversion happens here.
/// Out-of-range values are legal mid-pipeline (HDR headroom, out-of-gamut
/// Oklab results) and clamp on conversion back to bytes.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct LinearRgba {
    /// Red channel (linear light).
    pub r: f32,
    /// Green channel (linear light).
    pub g: f32,
    /// Blue channel (linear light).
    pub b: f32,
    /// Alpha (0.0 = transparent, 1.0 = opaque; never gamma encoded).
    pub a: f32,
}

impl LinearRgba {
    /// Create a linear-light color from individual channel values.
    #[must_use]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
}

impl Default for LinearRgba {
    fn default() -> Self {
        Self {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }
    }
}

/// Hue/saturation/value color. Hue is degrees; saturation and value are
/// `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Hsv {
    /// Hue angle in degrees, wrapped to `[0, 360)` at conversion entry.
    pub h: f32,
    /// Saturation (0.0–1.0).
    pub s: f32,
    /// Value / brightness (0.0–1.0).
    pub v: f32,
}

impl Hsv {
    /// Create an HSV color.
    #[must_use]
    pub const fn new(h: f32, s: f32, v: f32) -> Self {
        Self { h, s, v }
    }
}

/// Hue/saturation/lightness color. Hue is degrees; saturation and
/// lightness are `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Hsl {
    /// Hue angle in degrees, wrapped to `[0, 360)` at conversion entry.
    pub h: f32,
    /// Saturation (0.0–1.0).
    pub s: f32,
    /// Lightness (0.0–1.0).
    pub l: f32,
}

impl Hsl {
    /// Create an HSL color.
    #[must_use]
    pub const fn new(h: f32, s: f32, l: f32) -> Self {
        Self { h, s, l }
    }
}

/// Oklab perceptual color space (Björn Ottosson, 2020), alpha-bearing.
///
/// Perceptually uniform: linear interpolation here matches human
/// perception of color difference. Alpha rides along unmodified through
/// every conversion and interpolation.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Oklab {
    /// Perceived lightness (0.0 = black, 1.0 = white).
    pub l: f32,
    /// Green–red opponent channel.
    pub a: f32,
    /// Blue–yellow opponent channel.
    pub b: f32,
    /// Alpha (0.0–1.0).
    pub alpha: f32,
}

impl Oklab {
    /// Create an Oklab color.
    #[must_use]
    pub const fn new(l: f32, a: f32, b: f32, alpha: f32) -> Self {
        Self { l, a, b, alpha }
    }
}

impl Default for Oklab {
    fn default() -> Self {
        Self {
            l: 0.0,
            a: 0.0,
            b: 0.0,
            alpha: 1.0,
        }
    }
}

/// Oklch — the polar form of [`Oklab`] (lightness, chroma, hue), alpha-bearing.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Oklch {
    /// Perceived lightness (0.0 = black, 1.0 = white).
    pub l: f32,
    /// Chroma (0.0 = gray, higher = more vivid).
    pub c: f32,
    /// Hue angle in degrees, `[0, 360)`.
    pub h: f32,
    /// Alpha (0.0–1.0).
    pub alpha: f32,
}

impl Oklch {
    /// Create an Oklch color.
    #[must_use]
    pub const fn new(l: f32, c: f32, h: f32, alpha: f32) -> Self {
        Self { l, c, h, alpha }
    }
}

impl Default for Oklch {
    fn default() -> Self {
        Self {
            l: 0.0,
            c: 0.0,
            h: 0.0,
            alpha: 1.0,
        }
    }
}
