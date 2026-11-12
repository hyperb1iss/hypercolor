//! Mounting-rotation vocabulary for display-capable devices: the wire
//! tokens the daemon speaks, the labels the UI shows for them, and how a
//! preview turns a frame back to what the panel's viewer sees. Kept
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

/// How a preview lays out a display's frame so it reads the way the
/// mounted panel does. The daemon turns everything it sends against the
/// mount, so the frame alone is sideways or upside down; the preview
/// turns it with the mount, as the hardware is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountedPreview {
    /// Inline style for the preview's box: the footprint the mounted
    /// panel presents, which swaps its sides at a quarter turn.
    pub host_style: String,
    /// Inline style for the element holding the frame; empty when the
    /// panel is upright.
    pub frame_style: String,
}

/// The `(width, height)` a `width` x `height` panel presents once mounted
/// at `mount`: its own, with the sides swapped at a quarter turn.
#[must_use]
pub fn mounted_footprint(width: u32, height: u32, mount: DisplayRotation) -> (u32, u32) {
    let (width, height) = (width.max(1), height.max(1));
    match mount {
        DisplayRotation::Deg0 | DisplayRotation::Deg180 => (width, height),
        DisplayRotation::Deg90 | DisplayRotation::Deg270 => (height, width),
    }
}

/// The preview layout for a `width` x `height` panel mounted at `mount`.
#[must_use]
pub fn mounted_preview(width: u32, height: u32, mount: DisplayRotation) -> MountedPreview {
    let (width, height) = (width.max(1), height.max(1));
    let (across, down) = mounted_footprint(width, height, mount);
    let quarter_turn = |degrees: u16| {
        // The frame keeps its own shape inside the swapped footprint:
        // as wide as the box is tall, centered, then turned.
        let (w, h) = (f64::from(width), f64::from(height));
        MountedPreview {
            host_style: format!(
                "aspect-ratio: {across} / {down}; display: grid; place-items: center;"
            ),
            frame_style: format!(
                "width: {:.4}%; height: {:.4}%; transform: rotate({degrees}deg);",
                w / h * 100.0,
                h / w * 100.0,
            ),
        }
    };
    match mount {
        DisplayRotation::Deg0 => MountedPreview {
            host_style: format!("aspect-ratio: {across} / {down};"),
            frame_style: String::new(),
        },
        DisplayRotation::Deg90 => quarter_turn(90),
        DisplayRotation::Deg180 => MountedPreview {
            host_style: format!("aspect-ratio: {across} / {down};"),
            frame_style: "transform: rotate(180deg);".to_owned(),
        },
        DisplayRotation::Deg270 => quarter_turn(270),
    }
}
