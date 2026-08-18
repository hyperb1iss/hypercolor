//! The pixel blend kernel — the alpha-composable blend-mode subset.
//!
//! Authored, scene-level blend modes (`Replace`, `Tint`, `LumaReveal`, …)
//! live in `hypercolor-types` and map into this kernel at the compositor;
//! this crate never learns scene semantics.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::types::LinearRgba;

/// Pixel-kernel blend modes: exactly the alpha-composable set the
/// compositor's per-pixel kernel implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum PixelBlendMode {
    /// Source-over alpha compositing.
    #[default]
    Normal,
    /// `dst + src`, clamped to 1.0. Glow and flash.
    Add,
    /// `1 - (1-dst)(1-src)`. Brightens without blowing out.
    Screen,
    /// `dst * src`. Darkens; tinting.
    Multiply,
    /// Screen above mid-gray, Multiply below. Contrast.
    Overlay,
    /// Softer Overlay.
    SoftLight,
    /// `dst / (1 - src)`. Intense highlights.
    ColorDodge,
    /// `|dst - src|`. Psychedelic inversion.
    Difference,
}

impl LinearRgba {
    /// Blend `self` (the SOURCE) over `dst` (the destination) at
    /// `opacity`, returning the composited pixel.
    ///
    /// Source alpha is modulated by `opacity` before compositing; the
    /// result alpha is standard source-over union.
    #[must_use]
    pub fn blend_over(self, dst: LinearRgba, mode: PixelBlendMode, opacity: f32) -> LinearRgba {
        let a = self.a * opacity;
        let channel = |d: f32, s: f32| -> f32 {
            let blended = match mode {
                PixelBlendMode::Normal => s,
                PixelBlendMode::Add => (d + s).min(1.0),
                PixelBlendMode::Screen => 1.0 - (1.0 - d) * (1.0 - s),
                PixelBlendMode::Multiply => d * s,
                PixelBlendMode::Overlay => {
                    if d < 0.5 {
                        2.0 * d * s
                    } else {
                        1.0 - 2.0 * (1.0 - d) * (1.0 - s)
                    }
                }
                PixelBlendMode::SoftLight => {
                    if s < 0.5 {
                        d - (1.0 - 2.0 * s) * d * (1.0 - d)
                    } else {
                        d + (2.0 * s - 1.0) * (d.sqrt() - d)
                    }
                }
                PixelBlendMode::ColorDodge => {
                    if s >= 1.0 {
                        1.0
                    } else {
                        (d / (1.0 - s)).min(1.0)
                    }
                }
                PixelBlendMode::Difference => (d - s).abs(),
            };
            d.mul_add(1.0 - a, blended * a)
        };

        LinearRgba {
            r: channel(dst.r, self.r),
            g: channel(dst.g, self.g),
            b: channel(dst.b, self.b),
            a: (dst.a + a - dst.a * a).min(1.0),
        }
    }
}
