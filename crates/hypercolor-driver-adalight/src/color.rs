//! Byte order of one LED on the serial line.
//!
//! Hypercolor renders RGB. Adalight-family firmware either reorders bytes
//! for its strip itself or passes them through untouched, so the host
//! offers every permutation of the three channels and the user picks the
//! one that makes red show red.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Channel order of one LED in the payload.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum ColorOrder {
    /// Red, green, blue. What Adalight and HyperSerial firmware expect.
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
}

impl ColorOrder {
    /// Every supported order, in display order.
    pub const ALL: [Self; 6] = [
        Self::Rgb,
        Self::Rbg,
        Self::Grb,
        Self::Gbr,
        Self::Brg,
        Self::Bgr,
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

    /// Source RGB index for each of the three positions on the wire.
    const fn rgb_indices(self) -> [usize; 3] {
        match self {
            Self::Rgb => [0, 1, 2],
            Self::Rbg => [0, 2, 1],
            Self::Grb => [1, 0, 2],
            Self::Gbr => [1, 2, 0],
            Self::Brg => [2, 0, 1],
            Self::Bgr => [2, 1, 0],
        }
    }

    /// Reorder one RGB pixel into wire order.
    #[must_use]
    pub const fn apply(self, rgb: [u8; 3]) -> [u8; 3] {
        let [first, second, third] = self.rgb_indices();
        [rgb[first], rgb[second], rgb[third]]
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
            format!("unknown color order '{value}' (expected rgb, rbg, grb, gbr, brg, or bgr)")
        })
    }
}

impl From<ColorOrder> for String {
    fn from(value: ColorOrder) -> Self {
        value.as_str().to_owned()
    }
}
