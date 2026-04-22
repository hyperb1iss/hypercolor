//! Shared viewport primitives for effect controls and sampling.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::spatial::NormalizedRect;

/// Minimum normalized edge length allowed for a viewport selection.
pub const MIN_VIEWPORT_EDGE: f32 = 0.02;

/// Normalized viewport rectangle in `[0.0, 1.0]` source space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ViewportRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl ViewportRect {
    #[must_use]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub const fn full() -> Self {
        Self::new(0.0, 0.0, 1.0, 1.0)
    }

    #[must_use]
    pub fn clamp(self) -> Self {
        let width = if self.width.is_finite() {
            self.width.clamp(MIN_VIEWPORT_EDGE, 1.0)
        } else {
            1.0
        };
        let height = if self.height.is_finite() {
            self.height.clamp(MIN_VIEWPORT_EDGE, 1.0)
        } else {
            1.0
        };
        let x = if self.x.is_finite() {
            self.x.clamp(0.0, (1.0 - width).max(0.0))
        } else {
            0.0
        };
        let y = if self.y.is_finite() {
            self.y.clamp(0.0, (1.0 - height).max(0.0))
        } else {
            0.0
        };

        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::as_conversions
    )]
    pub fn to_pixel_rect(self, source_width: u32, source_height: u32) -> PixelRect {
        let rect = self.clamp();
        let source_width = source_width.max(1);
        let source_height = source_height.max(1);
        let source_width_f = source_width as f32;
        let source_height_f = source_height as f32;

        let left = (rect.x * source_width_f).floor() as u32;
        let top = (rect.y * source_height_f).floor() as u32;
        let right = ((rect.x + rect.width) * source_width_f)
            .ceil()
            .clamp(0.0, source_width_f) as u32;
        let bottom = ((rect.y + rect.height) * source_height_f)
            .ceil()
            .clamp(0.0, source_height_f) as u32;

        PixelRect {
            x: left.min(source_width.saturating_sub(1)),
            y: top.min(source_height.saturating_sub(1)),
            width: right.saturating_sub(left).max(1),
            height: bottom.saturating_sub(top).max(1),
        }
    }
}

impl Default for ViewportRect {
    fn default() -> Self {
        Self::full()
    }
}

impl From<ViewportRect> for NormalizedRect {
    fn from(value: ViewportRect) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

/// Pixel-space rectangle derived from a [`ViewportRect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// How a cropped viewport maps into a destination canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    #[default]
    Contain,
    Cover,
    Stretch,
}
