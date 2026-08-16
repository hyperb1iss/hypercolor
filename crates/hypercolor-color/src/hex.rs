//! Hex color parsing and formatting.
//!
//! Grammar: an optional single `#`, then exactly 3, 4, 6, or 8 hex
//! digits, with CSS shorthand expansion (`f80` → `ff8800`). Anything
//! else is an error — there are no silent fallback colors anywhere in
//! this crate; callers pick fallbacks explicitly.

use crate::types::{LinearRgba, Rgb, Rgba};

/// Why a hex color string failed to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ColorParseError {
    /// The digit count (after the optional `#`) is not one this parse
    /// accepts. Carries the digit count seen.
    #[error("hex color has {0} digits; expected 3, 4, 6, or 8")]
    BadLength(usize),
    /// A character was not a hex digit.
    #[error("hex color contains a non-hex digit")]
    BadDigit,
}

#[inline]
fn nibble(byte: u8) -> Result<u8, ColorParseError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(ColorParseError::BadDigit),
    }
}

#[inline]
fn pair(high: u8, low: u8) -> Result<u8, ColorParseError> {
    Ok((nibble(high)? << 4) | nibble(low)?)
}

/// Parse the digit body into RGBA channels. Shorthand forms imply
/// `a = 255` when no alpha digits are present.
fn parse_channels(s: &str) -> Result<Rgba, ColorParseError> {
    let digits = s.strip_prefix('#').unwrap_or(s).as_bytes();
    match digits.len() {
        3 => Ok(Rgba {
            r: pair(digits[0], digits[0])?,
            g: pair(digits[1], digits[1])?,
            b: pair(digits[2], digits[2])?,
            a: 255,
        }),
        4 => Ok(Rgba {
            r: pair(digits[0], digits[0])?,
            g: pair(digits[1], digits[1])?,
            b: pair(digits[2], digits[2])?,
            a: pair(digits[3], digits[3])?,
        }),
        6 => Ok(Rgba {
            r: pair(digits[0], digits[1])?,
            g: pair(digits[2], digits[3])?,
            b: pair(digits[4], digits[5])?,
            a: 255,
        }),
        8 => Ok(Rgba {
            r: pair(digits[0], digits[1])?,
            g: pair(digits[2], digits[3])?,
            b: pair(digits[4], digits[5])?,
            a: pair(digits[6], digits[7])?,
        }),
        n => Err(ColorParseError::BadLength(n)),
    }
}

impl Rgb {
    /// Parse a 3- or 6-digit hex color (optional leading `#`).
    ///
    /// Alpha-bearing forms (4/8 digits) are rejected with
    /// [`ColorParseError::BadLength`] — dropping alpha silently would be
    /// a loss; parse [`Rgba`] and decide explicitly instead.
    pub fn from_hex(s: &str) -> Result<Rgb, ColorParseError> {
        let digits = s.strip_prefix('#').unwrap_or(s).len();
        match digits {
            3 | 6 => Ok(parse_channels(s)?.to_rgb()),
            n => Err(ColorParseError::BadLength(n)),
        }
    }

    /// Format as a lowercase CSS hex color, `#rrggbb`.
    #[must_use]
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl Rgba {
    /// Parse a 3-, 4-, 6-, or 8-digit hex color (optional leading `#`).
    /// Forms without alpha digits imply full opacity.
    pub fn from_hex(s: &str) -> Result<Rgba, ColorParseError> {
        parse_channels(s)
    }
}

impl LinearRgba {
    /// Parse a hex color as encoded sRGB, then linearize.
    ///
    /// The name says the order: parse the bytes first, then decode the
    /// sRGB transfer — never the other way around.
    pub fn from_hex_srgb(s: &str) -> Result<LinearRgba, ColorParseError> {
        Ok(Rgba::from_hex(s)?.to_linear())
    }
}
