//! Mounting-rotation vocabulary for display-capable devices: the wire
//! tokens the daemon speaks and the labels the UI shows for them. Kept
//! leptos-free so `tests/display_rotation_tests.rs` can pin the table.

use hypercolor_types::scene::DisplayRotation;

/// Every rotation in picker order, with its wire token and label.
pub const DISPLAY_ROTATIONS: [(DisplayRotation, &str, &str); 4] = [
    (DisplayRotation::Deg0, "deg0", "Upright"),
    (DisplayRotation::Deg90, "deg90", "90° clockwise"),
    (DisplayRotation::Deg180, "deg180", "Upside down"),
    (DisplayRotation::Deg270, "deg270", "90° counter-clockwise"),
];

/// The wire token for a rotation, matching its serde spelling.
#[must_use]
pub fn display_rotation_value(rotation: DisplayRotation) -> &'static str {
    DISPLAY_ROTATIONS
        .iter()
        .find(|(candidate, _, _)| *candidate == rotation)
        .map_or("deg0", |(_, value, _)| value)
}

/// The user-facing label for a rotation.
#[must_use]
pub fn display_rotation_label(rotation: DisplayRotation) -> &'static str {
    DISPLAY_ROTATIONS
        .iter()
        .find(|(candidate, _, _)| *candidate == rotation)
        .map_or("Upright", |(_, _, label)| label)
}

/// Parse a wire token back into a rotation; unknown tokens read upright.
#[must_use]
pub fn parse_display_rotation(value: &str) -> DisplayRotation {
    DISPLAY_ROTATIONS
        .iter()
        .find(|(_, candidate, _)| *candidate == value.trim())
        .map_or(DisplayRotation::Deg0, |(rotation, _, _)| *rotation)
}

/// `(value, label)` pairs for a select control, in picker order.
#[must_use]
pub fn display_rotation_select_options() -> Vec<(String, String)> {
    DISPLAY_ROTATIONS
        .iter()
        .map(|(_, value, label)| ((*value).to_owned(), (*label).to_owned()))
        .collect()
}
