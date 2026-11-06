//! LED layout to spatial segments (Spec 92 §7).
//!
//! The device stores one coordinate per LED from the mapping the user ran
//! in the Twinkly app. Hypercolor keeps LED order (index `i` in the frame is
//! coordinate `i`) and turns the coordinates into normalized positions.

use hypercolor_types::device::{
    DeviceColorFormat, DeviceTopologyHint, SegmentInfo, SegmentLayoutHint,
};
use hypercolor_types::spatial::{LedTopology, NormalizedPosition};

use crate::api::{LayoutCoordinate, LayoutReply};

/// Spans narrower than this are treated as a single line on that axis.
const MIN_AXIS_SPAN: f64 = 1e-6;

/// How the layout was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutSource {
    /// No mapping: LEDs in wiring order along a line.
    Linear,
    /// Flat mapping, for example a wall or a matrix.
    TwoD,
    /// Three-dimensional mapping, for example a tree.
    ThreeD,
    /// No layout could be read.
    Unavailable,
}

impl LayoutSource {
    /// Parse the layout `source` field.
    #[must_use]
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim) {
            Some("2d") => Self::TwoD,
            Some("3d") => Self::ThreeD,
            Some("linear") => Self::Linear,
            _ => Self::Linear,
        }
    }

    /// Name used in metadata and diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::TwoD => "2d",
            Self::ThreeD => "3d",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Segments for a device with `led_count` LEDs and an optional layout.
///
/// A 2D or 3D layout with exactly one coordinate per LED becomes one
/// segment with custom positions (3D layouts are viewed from the front, so
/// depth is dropped). Anything else is a strip in wiring order.
#[must_use]
pub fn segments_for(
    led_count: u32,
    layout: Option<&LayoutReply>,
) -> (Vec<SegmentInfo>, LayoutSource) {
    let Some(layout) = layout else {
        return (vec![strip_segment(led_count)], LayoutSource::Unavailable);
    };
    let source = LayoutSource::parse(layout.source.as_deref());
    let mapped = matches!(source, LayoutSource::TwoD | LayoutSource::ThreeD);
    let count_matches =
        usize::try_from(led_count).is_ok_and(|count| count == layout.coordinates.len());
    if !mapped || !count_matches || led_count == 0 {
        return (vec![strip_segment(led_count)], source);
    }
    let positions = normalized_positions(&layout.coordinates);
    let segment = SegmentInfo {
        name: "LEDs".to_owned(),
        led_count,
        topology: DeviceTopologyHint::Custom,
        color_format: DeviceColorFormat::Rgb,
        layout_hint: Some(SegmentLayoutHint {
            topology: Some(LedTopology::Custom { positions }),
            ..SegmentLayoutHint::default()
        }),
    };
    (vec![segment], source)
}

/// One strip segment in wiring order.
#[must_use]
pub fn strip_segment(led_count: u32) -> SegmentInfo {
    SegmentInfo {
        name: "LEDs".to_owned(),
        led_count,
        topology: if led_count == 1 {
            DeviceTopologyHint::Point
        } else {
            DeviceTopologyHint::Strip
        },
        color_format: DeviceColorFormat::Rgb,
        layout_hint: None,
    }
}

/// Map layout coordinates into `[0, 1]` on both axes.
///
/// Each axis is scaled by its own extent so the LEDs fill the zone. The
/// Twinkly Y axis points up while canvas Y points down, so Y is flipped.
/// An axis with no extent (every LED on one vertical or horizontal line)
/// sits at the center.
#[must_use]
pub fn normalized_positions(coordinates: &[LayoutCoordinate]) -> Vec<NormalizedPosition> {
    let (min_x, max_x) = extent(coordinates.iter().map(|coordinate| coordinate.x));
    let (min_y, max_y) = extent(coordinates.iter().map(|coordinate| coordinate.y));
    coordinates
        .iter()
        .map(|coordinate| {
            let x = scale(coordinate.x, min_x, max_x);
            let y = 1.0 - scale(coordinate.y, min_y, max_y);
            NormalizedPosition::new(to_f32(x), to_f32(y))
        })
        .collect()
}

fn extent(values: impl Iterator<Item = f64>) -> (f64, f64) {
    values
        .filter(|value| value.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| {
            (low.min(value), high.max(value))
        })
}

fn scale(value: f64, min: f64, max: f64) -> f64 {
    let span = max - min;
    if !value.is_finite() || !span.is_finite() || span < MIN_AXIS_SPAN {
        return 0.5;
    }
    ((value - min) / span).clamp(0.0, 1.0)
}

/// Normalized coordinates lie in `[0, 1]`, well inside `f32` range.
const fn to_f32(value: f64) -> f32 {
    value as f32
}
