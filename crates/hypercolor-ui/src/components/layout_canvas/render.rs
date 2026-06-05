use crate::layout_geometry::ResizeHandle;
use hypercolor_types::spatial::ZoneShape;

/// Per-zone render data extracted from layout signal.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ZoneRenderData {
    pub(super) position_style: String,
    pub(super) primary_rgb: String,
    pub(super) secondary_rgb: String,
    pub(super) name: String,
    pub(super) led_count: u32,
    pub(super) shape: Option<ZoneShape>,
}

pub(super) fn zone_shape_style(shape: &Option<ZoneShape>) -> String {
    match shape {
        Some(ZoneShape::Ring) | Some(ZoneShape::Arc { .. }) => "border-radius: 999px".to_owned(),
        _ => String::new(),
    }
}

pub(super) fn ring_inner_style(
    shape: &Option<ZoneShape>,
    primary_rgb: &str,
    secondary_rgb: &str,
) -> Option<String> {
    match shape {
        Some(ZoneShape::Ring) => Some(format!(
            "border: 1px solid rgba({primary_rgb}, 0.16); \
             background: radial-gradient(circle, rgba(0, 0, 0, 0.5), rgba({secondary_rgb}, 0.04)); \
             box-shadow: inset 0 0 18px rgba(0, 0, 0, 0.45)"
        )),
        _ => None,
    }
}

/// Compute the CSS cursor for a resize handle, accounting for zone rotation.
///
/// Each handle has a base angle (NW=315°, NE=45°, SE=135°, SW=225°). We add
/// the zone rotation, then snap to the nearest 45° cursor direction.
pub(super) fn rotated_cursor(handle: ResizeHandle, rotation_deg: f32) -> &'static str {
    let base = match handle {
        ResizeHandle::NorthWest => 315.0,
        ResizeHandle::NorthEast => 45.0,
        ResizeHandle::SouthEast => 135.0,
        ResizeHandle::SouthWest => 225.0,
    };
    let effective = (base + rotation_deg).rem_euclid(360.0);
    let sector = ((effective + 22.5) / 45.0) as u32 % 8;
    match sector {
        0 => "n-resize",
        1 => "ne-resize",
        2 => "e-resize",
        3 => "se-resize",
        4 => "s-resize",
        5 => "sw-resize",
        6 => "w-resize",
        7 => "nw-resize",
        _ => "nw-resize",
    }
}
