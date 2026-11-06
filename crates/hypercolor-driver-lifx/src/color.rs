//! HSBK values and the engine RGB to HSBK conversion (Spec 88 §8).
//!
//! Hue follows the HSV wheel of the sRGB-encoded bytes, which is the wheel
//! every LIFX client sends. Saturation is measured in linear light, so the
//! share of white light matches the source pixel and kelvin can never wash
//! out a saturated color. Brightness is the largest encoded channel.

use hypercolor_color::{Hsv, LUMA_B, LUMA_G, LUMA_R, Rgb, linear_to_srgb, lut};

/// sRGB's D65 white point, the default kelvin for desaturated colors.
pub const DEFAULT_WHITE_KELVIN: u16 = 6500;
/// Narrowest kelvin bounds accepted by the driver configuration.
pub const MIN_CONFIG_KELVIN: u16 = 1500;
/// Widest kelvin bound accepted by the driver configuration.
pub const MAX_CONFIG_KELVIN: u16 = 9000;

/// One LIFX color: hue, saturation, brightness, kelvin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Hsbk {
    /// 0-65535 over 0-360 degrees.
    pub hue: u16,
    /// 0-65535 over 0-1.
    pub saturation: u16,
    /// 0-65535 over 0-1.
    pub brightness: u16,
    /// White point in kelvin.
    pub kelvin: u16,
}

impl Hsbk {
    #[must_use]
    pub const fn new(hue: u16, saturation: u16, brightness: u16, kelvin: u16) -> Self {
        Self {
            hue,
            saturation,
            brightness,
            kelvin,
        }
    }

    /// Wire encoding, four little-endian u16 values.
    #[must_use]
    pub fn to_le_bytes(self) -> [u8; 8] {
        let mut bytes = [0_u8; 8];
        bytes[0..2].copy_from_slice(&self.hue.to_le_bytes());
        bytes[2..4].copy_from_slice(&self.saturation.to_le_bytes());
        bytes[4..6].copy_from_slice(&self.brightness.to_le_bytes());
        bytes[6..8].copy_from_slice(&self.kelvin.to_le_bytes());
        bytes
    }

    /// Decode the wire encoding.
    #[must_use]
    pub fn from_le_bytes(bytes: [u8; 8]) -> Self {
        Self {
            hue: u16::from_le_bytes([bytes[0], bytes[1]]),
            saturation: u16::from_le_bytes([bytes[2], bytes[3]]),
            brightness: u16::from_le_bytes([bytes[4], bytes[5]]),
            kelvin: u16::from_le_bytes([bytes[6], bytes[7]]),
        }
    }
}

/// Convert hue degrees to the LIFX wire scale, `round(65536 * deg / 360)
/// mod 65536`.
#[must_use]
pub fn hue_degrees_to_u16(degrees: f32) -> u16 {
    let wrapped = hypercolor_color::wrap_hue(degrees);
    let scaled = (wrapped / 360.0 * 65_536.0).round();
    let raw = scaled as u32;
    u16::try_from(raw % 65_536).unwrap_or(0)
}

/// Convert a unit value in `[0, 1]` to the LIFX `0..=65535` scale.
#[must_use]
pub fn unit_to_u16(value: f32) -> u16 {
    let scaled = (value.clamp(0.0, 1.0) * 65_535.0).round();
    let raw = scaled as u32;
    u16::try_from(raw).unwrap_or(u16::MAX)
}

/// Per-device RGB to HSBK conversion settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HsbkEncoder {
    /// Kelvin for every output color, already clamped to the product range.
    pub kelvin: u16,
    /// Whether the device renders hue and saturation.
    pub color_capable: bool,
    /// Device brightness scale from the host, 255 is full.
    pub brightness: u8,
}

impl HsbkEncoder {
    #[must_use]
    pub const fn new(kelvin: u16, color_capable: bool) -> Self {
        Self {
            kelvin,
            color_capable,
            brightness: u8::MAX,
        }
    }

    /// Convert one sRGB-encoded pixel.
    #[must_use]
    pub fn encode(&self, [r, g, b]: [u8; 3]) -> Hsbk {
        let max_encoded = r.max(g).max(b);
        if max_encoded == 0 {
            return Hsbk::new(0, 0, 0, self.kelvin);
        }

        let red = lut::srgb_u8_to_linear(r);
        let green = lut::srgb_u8_to_linear(g);
        let blue = lut::srgb_u8_to_linear(b);

        if !self.color_capable {
            let luminance = LUMA_R * red + LUMA_G * green + LUMA_B * blue;
            let brightness = unit_to_u16(linear_to_srgb(luminance));
            return Hsbk::new(0, 0, self.scale_brightness(brightness), self.kelvin);
        }

        let max_linear = red.max(green).max(blue);
        let min_linear = red.min(green).min(blue);
        let saturation = if max_linear <= 0.0 {
            0.0
        } else {
            (max_linear - min_linear) / max_linear
        };
        let hue = Hsv::from_rgb(Rgb::new(r, g, b)).h;
        let brightness = u16::from(max_encoded) * 257;

        Hsbk::new(
            hue_degrees_to_u16(hue),
            unit_to_u16(saturation),
            self.scale_brightness(brightness),
            self.kelvin,
        )
    }

    fn scale_brightness(&self, brightness: u16) -> u16 {
        if self.brightness == u8::MAX {
            return brightness;
        }
        let scaled = (u32::from(brightness) * u32::from(self.brightness) + 127) / 255;
        u16::try_from(scaled).unwrap_or(u16::MAX)
    }
}

/// Clamp a configured kelvin into a product's supported range.
#[must_use]
pub fn clamp_kelvin(kelvin: u16, range: (u16, u16)) -> u16 {
    let (low, high) = if range.0 <= range.1 {
        range
    } else {
        (range.1, range.0)
    };
    kelvin.clamp(low, high)
}
