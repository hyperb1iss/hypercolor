//! Channel order encoding for pixel controllers and DMX fixtures.
//!
//! Hypercolor renders RGB. A controller port or fixture expects its own
//! channel order, optionally with a white channel and extra trailing
//! channels per pixel. This module writes the device's channel stream.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Channel order of one pixel on the wire.
///
/// Four-channel orders carry white last. White is extracted as the shared
/// minimum of red, green, and blue, which is then subtracted from each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum ColorOrder {
    /// Red, green, blue.
    #[default]
    Rgb,
    /// Red, blue, green.
    Rbg,
    /// Green, red, blue.
    Grb,
    /// Green, blue, red.
    Gbr,
    /// Blue, red, green.
    Brg,
    /// Blue, green, red.
    Bgr,
    /// Red, green, blue, white.
    Rgbw,
    /// Red, blue, green, white.
    Rbgw,
    /// Green, red, blue, white.
    Grbw,
    /// Green, blue, red, white.
    Gbrw,
    /// Blue, red, green, white.
    Brgw,
    /// Blue, green, red, white.
    Bgrw,
}

impl ColorOrder {
    /// Every supported order, in display order.
    pub const ALL: [Self; 12] = [
        Self::Rgb,
        Self::Rbg,
        Self::Grb,
        Self::Gbr,
        Self::Brg,
        Self::Bgr,
        Self::Rgbw,
        Self::Rbgw,
        Self::Grbw,
        Self::Gbrw,
        Self::Brgw,
        Self::Bgrw,
    ];

    /// Stable lowercase identifier used in config.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rgb => "rgb",
            Self::Rbg => "rbg",
            Self::Grb => "grb",
            Self::Gbr => "gbr",
            Self::Brg => "brg",
            Self::Bgr => "bgr",
            Self::Rgbw => "rgbw",
            Self::Rbgw => "rbgw",
            Self::Grbw => "grbw",
            Self::Gbrw => "gbrw",
            Self::Brgw => "brgw",
            Self::Bgrw => "bgrw",
        }
    }

    /// Parse an order name, ignoring ASCII case.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL
            .into_iter()
            .find(|order| order.as_str().eq_ignore_ascii_case(value))
    }

    /// Color channels per pixel: 3, or 4 with white.
    #[must_use]
    pub const fn color_channels(self) -> usize {
        if self.has_white() { 4 } else { 3 }
    }

    /// Whether this order carries a white channel.
    #[must_use]
    pub const fn has_white(self) -> bool {
        matches!(
            self,
            Self::Rgbw | Self::Rbgw | Self::Grbw | Self::Gbrw | Self::Brgw | Self::Bgrw
        )
    }

    /// Source RGB index for each of the three color positions on the wire.
    const fn rgb_indices(self) -> [usize; 3] {
        match self {
            Self::Rgb | Self::Rgbw => [0, 1, 2],
            Self::Rbg | Self::Rbgw => [0, 2, 1],
            Self::Grb | Self::Grbw => [1, 0, 2],
            Self::Gbr | Self::Gbrw => [1, 2, 0],
            Self::Brg | Self::Brgw => [2, 0, 1],
            Self::Bgr | Self::Bgrw => [2, 1, 0],
        }
    }

    /// Encode one RGB pixel into `out`, which must hold `color_channels()` bytes.
    pub fn encode_into(self, rgb: [u8; 3], out: &mut [u8]) {
        let [r_index, g_index, b_index] = self.rgb_indices();
        if self.has_white() {
            let white = rgb[0].min(rgb[1]).min(rgb[2]);
            out[0] = rgb[r_index] - white;
            out[1] = rgb[g_index] - white;
            out[2] = rgb[b_index] - white;
            out[3] = white;
        } else {
            out[0] = rgb[r_index];
            out[1] = rgb[g_index];
            out[2] = rgb[b_index];
        }
    }
}

impl fmt::Display for ColorOrder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TryFrom<String> for ColorOrder {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value).ok_or_else(|| {
            format!(
                "unknown color order '{value}' (expected one of {})",
                Self::ALL.map(Self::as_str).join(", ")
            )
        })
    }
}

impl From<ColorOrder> for String {
    fn from(value: ColorOrder) -> Self {
        value.as_str().to_owned()
    }
}

/// Encode a frame into the device's channel stream.
///
/// Writes exactly `pixel_count * channels_per_pixel` bytes into `out`.
/// Missing pixels are black, extra input pixels are ignored, and channels
/// past the color channels of each pixel are written as zero.
pub fn encode_pixels(
    colors: &[[u8; 3]],
    order: ColorOrder,
    channels_per_pixel: usize,
    pixel_count: usize,
    out: &mut Vec<u8>,
) {
    let stride = channels_per_pixel.max(order.color_channels());
    out.clear();
    out.resize(pixel_count * stride, 0);
    for (pixel, chunk) in out.chunks_exact_mut(stride).enumerate() {
        let rgb = colors.get(pixel).copied().unwrap_or([0, 0, 0]);
        order.encode_into(rgb, chunk);
    }
}
