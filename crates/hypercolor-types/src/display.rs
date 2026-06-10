//! Display surface description shared by the daemon, the Servo face
//! bootstrap, and (via injection) the face SDK.
//!
//! A [`DisplayDescriptor`] is computed once per device from driver topology
//! hints and handed to the face renderer before page boot. The JS-facing
//! view is versioned and additive-only: fields may be added, never removed
//! or renamed.

use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

/// Current `window.hypercolor.display` contract version.
pub const DISPLAY_DESCRIPTOR_API_VERSION: u32 = 1;

/// Aspect ratio at or above which a display counts as [`DisplayShape::Wide`].
pub const WIDE_ASPECT_THRESHOLD: f64 = 2.0;

/// Broad shape classification a face adapts its layout to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DisplayShape {
    /// Circular panel (e.g., Corsair pump cap LCD).
    Round,
    /// Roughly 1:1 rectangular panel.
    Square,
    /// Aspect ratio >= 2:1 (e.g., Push 2's 960x160 strip).
    Wide,
    /// Aspect ratio <= 1:2.
    Tall,
}

/// Device family the display belongs to, for layout idiom selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DisplayClass {
    /// Small round/square LCD on a cooler pump cap.
    PumpLcd,
    /// Long thin display strip above controls.
    Strip,
    /// General-purpose rectangular panel.
    Panel,
}

/// Pixel rectangle within a display surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub struct DisplayRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl DisplayRect {
    #[must_use]
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The full surface rect for a display of the given dimensions.
    #[must_use]
    pub const fn full(width: u32, height: u32) -> Self {
        Self::new(0, 0, width, height)
    }
}

/// Pixel format the device transport expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DisplayPixelFormat {
    Rgb,
    Yuv420,
}

/// Everything a face needs to know about the surface it renders on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DisplayDescriptor {
    /// Contract version for the injected JS view; additive-only.
    pub api_version: u32,
    pub width: u32,
    pub height: u32,
    pub circular: bool,
    pub shape: DisplayShape,
    pub class: DisplayClass,
    /// Largest rect guaranteed free of physical clipping (the inscribed
    /// square on round panels, the full surface otherwise).
    pub safe_area: DisplayRect,
    pub target_fps: u32,
    pub pixel_format: DisplayPixelFormat,
}

impl DisplayDescriptor {
    /// Derive a descriptor from raw surface facts and an optional driver
    /// class hint. Pure; zero dimensions are clamped to one pixel.
    #[must_use]
    pub fn derive(
        width: u32,
        height: u32,
        circular: bool,
        class_hint: Option<DisplayClass>,
        target_fps: u32,
        pixel_format: DisplayPixelFormat,
    ) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let shape = derive_shape(width, height, circular);
        let class = class_hint.unwrap_or_else(|| default_class(shape));
        let safe_area = derive_safe_area(width, height, shape);

        Self {
            api_version: DISPLAY_DESCRIPTOR_API_VERSION,
            width,
            height,
            circular,
            shape,
            class,
            safe_area,
            target_fps,
            pixel_format,
        }
    }

    /// Width-over-height aspect ratio.
    #[must_use]
    #[allow(clippy::cast_lossless)]
    pub fn aspect(&self) -> f64 {
        f64::from(self.width) / f64::from(self.height.max(1))
    }

    /// The camelCase JSON view injected as `window.hypercolor.display`.
    ///
    /// This is the SDK-facing wire contract from spec 69 §3.1; key names
    /// here are frozen per `api_version`.
    #[must_use]
    pub fn bootstrap_json(&self) -> serde_json::Value {
        json!({
            "apiVersion": self.api_version,
            "width": self.width,
            "height": self.height,
            "circular": self.circular,
            "shape": shape_token(self.shape),
            "class": class_token(self.class),
            "safeArea": {
                "x": self.safe_area.x,
                "y": self.safe_area.y,
                "width": self.safe_area.width,
                "height": self.safe_area.height,
            },
            "targetFps": self.target_fps,
            "pixelFormat": pixel_format_token(self.pixel_format),
        })
    }
}

const fn shape_token(shape: DisplayShape) -> &'static str {
    match shape {
        DisplayShape::Round => "round",
        DisplayShape::Square => "square",
        DisplayShape::Wide => "wide",
        DisplayShape::Tall => "tall",
    }
}

const fn class_token(class: DisplayClass) -> &'static str {
    match class {
        DisplayClass::PumpLcd => "pump-lcd",
        DisplayClass::Strip => "strip",
        DisplayClass::Panel => "panel",
    }
}

const fn pixel_format_token(format: DisplayPixelFormat) -> &'static str {
    match format {
        DisplayPixelFormat::Rgb => "rgb",
        DisplayPixelFormat::Yuv420 => "yuv420",
    }
}

fn derive_shape(width: u32, height: u32, circular: bool) -> DisplayShape {
    if circular {
        return DisplayShape::Round;
    }

    let aspect = f64::from(width) / f64::from(height);
    if aspect >= WIDE_ASPECT_THRESHOLD {
        DisplayShape::Wide
    } else if aspect <= 1.0 / WIDE_ASPECT_THRESHOLD {
        DisplayShape::Tall
    } else {
        DisplayShape::Square
    }
}

/// Layout-idiom default when a driver declares no class: round panels are
/// overwhelmingly pump caps, long strips behave like strips, the rest are
/// generic panels.
const fn default_class(shape: DisplayShape) -> DisplayClass {
    match shape {
        DisplayShape::Round => DisplayClass::PumpLcd,
        DisplayShape::Wide | DisplayShape::Tall => DisplayClass::Strip,
        DisplayShape::Square => DisplayClass::Panel,
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn derive_safe_area(width: u32, height: u32, shape: DisplayShape) -> DisplayRect {
    if shape != DisplayShape::Round {
        return DisplayRect::full(width, height);
    }

    let diameter = f64::from(width.min(height));
    let side = (diameter / std::f64::consts::SQRT_2).floor() as u32;
    let side = side.max(1);
    let x = (width.saturating_sub(side)) / 2;
    let y = (height.saturating_sub(side)) / 2;
    DisplayRect::new(x, y, side, side)
}
