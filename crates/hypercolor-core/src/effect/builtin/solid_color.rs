//! Solid color renderer — fills the entire canvas with a single color.
//!
//! The simplest possible effect, extended with a few utility scene layouts
//! so it also works as a diagnostic and quick composition tool.

use hypercolor_types::canvas::{Canvas, RgbaF32};
use hypercolor_types::effect::{ControlValue, EffectMetadata};

use crate::effect::traits::{EffectRenderer, FrameInput};

/// Utility scene patterns layered on top of the solid fill renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SolidPattern {
    Solid,
    VerticalSplit,
    HorizontalSplit,
    Checker,
    Quadrants,
}

impl SolidPattern {
    fn from_str(value: &str) -> Self {
        match normalize_choice(value).as_str() {
            "vertical_split" => Self::VerticalSplit,
            "horizontal_split" => Self::HorizontalSplit,
            "checker" => Self::Checker,
            "quadrants" => Self::Quadrants,
            _ => Self::Solid,
        }
    }
}

/// Fills every pixel with a single color, optionally arranged into simple patterns.
pub struct SolidColorRenderer {
    /// Current color in normalized RGBA.
    color: [f32; 4],
    /// Secondary scene color for split and checker patterns.
    secondary_color: [f32; 4],
    /// Brightness multiplier (0.0 = black, 1.0 = full color).
    brightness: f32,
    /// Pattern mode for quick utility scenes and diagnostics.
    pattern: SolidPattern,
    /// Boundary position used by split and quadrant patterns.
    position: f32,
    /// Feather width for split patterns.
    softness: f32,
    /// Pattern scale used by checker mode.
    scale: f32,
}

impl SolidColorRenderer {
    /// Create a new solid color renderer with opaque white at full brightness.
    #[must_use]
    pub fn new() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 1.0],
            secondary_color: [0.0, 0.0, 0.0, 1.0],
            brightness: 1.0,
            pattern: SolidPattern::Solid,
            position: 0.5,
            softness: 0.0,
            scale: 6.0,
        }
    }

    fn pattern_mix(&self, nx: f32, ny: f32, width: f32, height: f32) -> f32 {
        match self.pattern {
            SolidPattern::Solid => 0.0,
            SolidPattern::VerticalSplit => transition_mix(nx, self.position, self.softness),
            SolidPattern::HorizontalSplit => transition_mix(ny, self.position, self.softness),
            SolidPattern::Checker => {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::as_conversions
                )]
                let cols = self.scale.max(1.0).round() as i32;
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::as_conversions
                )]
                let rows = (self.scale.max(1.0) * (height / width)).max(1.0).round() as i32;
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::as_conversions
                )]
                let col = (nx * cols as f32).floor() as i32;
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::as_conversions
                )]
                let row = (ny * rows as f32).floor() as i32;
                if (col + row) % 2 == 0 { 0.0 } else { 1.0 }
            }
            SolidPattern::Quadrants => {
                let right = transition_mix(nx, self.position, self.softness) >= 0.5;
                let bottom = transition_mix(ny, self.position, self.softness) >= 0.5;
                if right ^ bottom { 1.0 } else { 0.0 }
            }
        }
    }
}

impl Default for SolidColorRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRenderer for SolidColorRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
    fn tick(&mut self, input: &FrameInput<'_>) -> anyhow::Result<Canvas> {
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);
        let width = input.canvas_width.max(1) as f32;
        let height = input.canvas_height.max(1) as f32;
        let primary = RgbaF32::new(self.color[0], self.color[1], self.color[2], self.color[3]);
        let secondary = RgbaF32::new(
            self.secondary_color[0],
            self.secondary_color[1],
            self.secondary_color[2],
            self.secondary_color[3],
        );

        for y in 0..input.canvas_height {
            let ny = (y as f32 + 0.5) / height;
            for x in 0..input.canvas_width {
                let nx = (x as f32 + 0.5) / width;
                let mix = self.pattern_mix(nx, ny, width, height);
                let mut pixel = RgbaF32::lerp(&primary, &secondary, mix);
                pixel.r *= self.brightness;
                pixel.g *= self.brightness;
                pixel.b *= self.brightness;
                canvas.set_pixel(x, y, pixel.to_srgba());
            }
        }

        Ok(canvas)
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "color" => {
                if let ControlValue::Color(c) = value {
                    self.color = *c;
                }
            }
            "secondary_color" => {
                if let ControlValue::Color(c) = value {
                    self.secondary_color = *c;
                }
            }
            "brightness" => {
                if let Some(v) = value.as_f32() {
                    self.brightness = v.clamp(0.0, 1.0);
                }
            }
            "pattern" => {
                if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value {
                    self.pattern = SolidPattern::from_str(choice);
                }
            }
            "position" => {
                if let Some(v) = value.as_f32() {
                    self.position = v.clamp(0.0, 1.0);
                }
            }
            "softness" => {
                if let Some(v) = value.as_f32() {
                    self.softness = v.clamp(0.0, 0.5);
                }
            }
            "scale" => {
                if let Some(v) = value.as_f32() {
                    self.scale = v.max(1.0);
                }
            }
            _ => {}
        }
    }

    fn destroy(&mut self) {}
}

fn transition_mix(value: f32, pivot: f32, softness: f32) -> f32 {
    if softness <= f32::EPSILON {
        return if value >= pivot { 1.0 } else { 0.0 };
    }

    let start = (pivot - softness).max(0.0);
    let end = (pivot + softness).min(1.0);
    smoothstep(start, end, value)
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    if (edge1 - edge0).abs() <= f32::EPSILON {
        return if value >= edge1 { 1.0 } else { 0.0 };
    }

    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn normalize_choice(value: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_separator = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            normalized.push('_');
            last_was_separator = true;
        }
    }

    normalized.trim_matches('_').to_owned()
}
