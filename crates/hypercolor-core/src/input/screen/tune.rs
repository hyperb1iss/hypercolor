//! Color tuning — saturation, brightness, and gamma shaping for zone colors.
//!
//! Applied after temporal smoothing, this is the "make it pop" stage of the
//! ambilight pipeline: screen content tends to look washed out when projected
//! onto LEDs, so a saturation boost and gamma shaping restore the punch.
//! All math runs in linear light to avoid hue shifts.

use crate::types::canvas::{linear_to_srgb_u8, srgb_u8_to_linear};

/// BT.709 luma coefficients in linear light.
const LUMA_R: f32 = 0.2126;
const LUMA_G: f32 = 0.7152;
const LUMA_B: f32 = 0.0722;

#[derive(Clone, Copy, Debug)]
pub(super) struct PreparedLinearColorTuning {
    saturation: f32,
    brightness: f32,
    gamma: f32,
    neutral: bool,
}

impl PreparedLinearColorTuning {
    pub(super) fn new(saturation: f32, brightness: f32, gamma: f32) -> Self {
        let tuning = ColorTuning {
            saturation,
            brightness,
            gamma,
        }
        .clamped();
        Self {
            saturation: tuning.saturation,
            brightness: tuning.brightness,
            gamma: tuning.gamma,
            neutral: tuning.is_neutral(),
        }
    }

    pub(super) const fn is_neutral(self) -> bool {
        self.neutral
    }

    pub(super) fn apply(self, color: &mut [f32; 3]) {
        if self.neutral {
            return;
        }
        let luma = LUMA_R * color[0] + LUMA_G * color[1] + LUMA_B * color[2];
        for channel in color {
            *channel = luma + (*channel - luma) * self.saturation;
            *channel = (*channel * self.brightness).max(0.0);
            *channel = channel.powf(self.gamma);
        }
    }
}

/// Saturation, brightness, and gamma adjustments for ambilight zone colors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorTuning {
    /// Saturation scale around per-pixel luma (1.0 = neutral, 0.0 = grayscale).
    pub saturation: f32,

    /// Linear brightness multiplier (1.0 = neutral).
    pub brightness: f32,

    /// Gamma exponent on linear channels (1.0 = neutral, >1 darkens mids).
    pub gamma: f32,
}

impl ColorTuning {
    /// Clamp each parameter to its sane operating range.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            saturation: self.saturation.clamp(0.0, 4.0),
            brightness: self.brightness.clamp(0.0, 4.0),
            gamma: self.gamma.clamp(0.2, 5.0),
        }
    }

    /// Whether this tuning changes nothing and can be skipped.
    #[must_use]
    pub fn is_neutral(&self) -> bool {
        const EPSILON: f32 = 1e-4;
        (self.saturation - 1.0).abs() < EPSILON
            && (self.brightness - 1.0).abs() < EPSILON
            && (self.gamma - 1.0).abs() < EPSILON
    }

    /// Apply this tuning to a set of zone colors in-place.
    pub fn apply(&self, colors: &mut [[u8; 3]]) {
        let tuning = PreparedLinearColorTuning::new(self.saturation, self.brightness, self.gamma);
        if tuning.is_neutral() {
            return;
        }

        for color in colors {
            let mut rgb = [
                srgb_u8_to_linear(color[0]),
                srgb_u8_to_linear(color[1]),
                srgb_u8_to_linear(color[2]),
            ];

            tuning.apply(&mut rgb);

            color[0] = linear_to_srgb_u8(rgb[0].clamp(0.0, 1.0));
            color[1] = linear_to_srgb_u8(rgb[1].clamp(0.0, 1.0));
            color[2] = linear_to_srgb_u8(rgb[2].clamp(0.0, 1.0));
        }
    }
}

impl Default for ColorTuning {
    fn default() -> Self {
        Self {
            saturation: 1.0,
            brightness: 1.0,
            gamma: 1.0,
        }
    }
}
