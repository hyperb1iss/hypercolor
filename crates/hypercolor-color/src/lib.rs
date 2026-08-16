//! Color kernel for the Hypercolor engine.
//!
//! This crate is the single canonical home for color math across the
//! workspace: pixel value types, color-space conversions, hex parsing,
//! pixel blending, brightness scaling, and device channel encoding.
//! It sits at the bottom of the crate graph with no runtime dependencies;
//! `hypercolor-types` re-exports the pixel data carriers.
//!
//! Normative conventions (Spec 76 §1.1):
//!
//! - Hue is degrees, wrapped `rem_euclid(360.0)` at every entry point.
//! - Saturation, value, lightness, and alpha are `0.0..=1.0`.
//! - Float→u8 conversion is always `(x * 255.0).round().clamp(0.0, 255.0)`.
//! - Linearization appears in the name; nothing converts color space
//!   implicitly.
//! - Value types are `Copy + PartialEq + Debug`; serde support is behind
//!   the `serde` feature, OpenAPI schemas behind `schema`.

mod blend;
mod convert;
mod encode;
mod hex;
mod transfer;
mod types;

pub use blend::PixelBlendMode;
pub use encode::{DevicePixelLayout, EncodedChannels};
pub use hex::ColorParseError;
pub use transfer::{linear_to_srgb, lut, srgb_to_linear};
pub use types::{Hsl, Hsv, LinearRgba, Oklab, Oklch, Rgb, Rgba};

/// BT.709 red luma coefficient.
pub const LUMA_R: f32 = 0.2126;
/// BT.709 green luma coefficient.
pub const LUMA_G: f32 = 0.7152;
/// BT.709 blue luma coefficient.
pub const LUMA_B: f32 = 0.0722;

/// Wrap a hue angle in degrees into `[0.0, 360.0)`.
///
/// `rem_euclid` alone can round back up to exactly `360.0` for inputs a
/// hair below zero, so the result is folded once more.
#[inline]
#[must_use]
pub fn wrap_hue(h: f32) -> f32 {
    let wrapped = h.rem_euclid(360.0);
    if wrapped >= 360.0 { 0.0 } else { wrapped }
}
