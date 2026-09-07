//! Device channel encoding and brightness scaling.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::types::Rgb;

/// Physical channel layout a device consumes.
///
/// `RgbwZeroWhite` is four channels with the white byte fixed at zero —
/// the honest name for what every current RGBW sink does. Real white
/// extraction is future work with its own variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum DevicePixelLayout {
    /// Red, green, blue.
    #[default]
    Rgb,
    /// Green, red, blue.
    Grb,
    /// Red, blue, green.
    Rbg,
    /// Red, green, blue, then a white channel always written as zero.
    RgbwZeroWhite,
}

impl DevicePixelLayout {
    /// Bytes one pixel occupies in this layout.
    #[must_use]
    pub const fn channel_count(self) -> u8 {
        match self {
            Self::Rgb | Self::Grb | Self::Rbg => 3,
            Self::RgbwZeroWhite => 4,
        }
    }
}

/// One pixel encoded for a device: the bytes and how many are valid.
///
/// Buffer length lives in the type, not in caller discipline — slice
/// with [`Self::as_slice`], never assume a channel count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedChannels {
    /// Channel bytes in device order; only the first `len` are valid.
    pub bytes: [u8; 4],
    /// Number of valid bytes (3 or 4).
    pub len: u8,
}

impl EncodedChannels {
    /// The valid channel bytes.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

impl AsRef<[u8]> for EncodedChannels {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Rgb {
    /// THE brightness scaler: multiply each channel by `factor`, round,
    /// clamp to byte range. Negative factors clamp to black; factors
    /// above 1.0 saturate at white.
    #[must_use]
    pub fn scale(self, factor: f32) -> Rgb {
        let channel = |c: u8| (f32::from(c) * factor).round().clamp(0.0, 255.0) as u8;
        Rgb {
            r: channel(self.r),
            g: channel(self.g),
            b: channel(self.b),
        }
    }

    /// Encode this pixel into a device channel layout.
    #[must_use]
    pub const fn encode(self, layout: DevicePixelLayout) -> EncodedChannels {
        let (bytes, len) = match layout {
            DevicePixelLayout::Rgb => ([self.r, self.g, self.b, 0], 3),
            DevicePixelLayout::Grb => ([self.g, self.r, self.b, 0], 3),
            DevicePixelLayout::Rbg => ([self.r, self.b, self.g, 0], 3),
            DevicePixelLayout::RgbwZeroWhite => ([self.r, self.g, self.b, 0], 4),
        };
        EncodedChannels { bytes, len }
    }
}
