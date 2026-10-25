//! When a box on the canvas shows its name.
//!
//! A dense rig turns into a wall of text when every box names itself, so a
//! box names itself at rest only when its name fits inside it and nothing
//! painted above covers the name. Otherwise the name waits for hover or
//! selection, and a box too small to hold it names itself in a callout
//! beside the box instead.

use std::collections::HashMap;

/// Estimated advance of one character of a box name, in pixels.
const NAME_CHAR_WIDTH: f32 = 6.4;
/// One line of a box name, in pixels.
const NAME_LINE_HEIGHT: f32 = 14.0;
/// The LED count line under the name, in pixels.
const DETAIL_HEIGHT: f32 = 15.0;
/// Inset between a box edge and its label, in pixels.
const LABEL_INSET: f32 = 4.0;
/// A name wraps onto at most this many lines.
const MAX_NAME_LINES: f32 = 2.0;

/// One drawn box, in viewport pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelBox {
    pub id: String,
    pub center: (f32, f32),
    /// Drawn width and height in the box's own frame, scale included.
    pub size: (f32, f32),
    pub rotation: f32,
    /// Characters in the box's name.
    pub name_chars: usize,
    /// Paint order; higher paints on top.
    pub z: usize,
}

/// How much room a box offers its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelRoom {
    /// The name fits, and nothing painted above covers it.
    Clear,
    /// The name fits, but a box painted above overlaps it.
    Covered,
    /// The box cannot hold its name.
    TooSmall,
}

/// Whether a label shows, shows only while the pointer rests on its box,
/// or stays hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Shown,
    OnHover,
    Hidden,
}

/// Where a box's name shows: inside the box, or in the callout beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelMode {
    pub inside: Visibility,
    pub callout: Visibility,
}

/// The label a box of `width` by `height` pixels needs for a name of
/// `chars` characters, or `None` when the box cannot hold the name on two
/// lines above its LED count.
#[must_use]
pub fn label_extent(chars: usize, width: f32, height: f32) -> Option<(f32, f32)> {
    let inner_width = width - 2.0 * LABEL_INSET;
    let inner_height = height - 2.0 * LABEL_INSET;
    if inner_width <= 0.0 || inner_height <= 0.0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss, reason = "names are short")]
    let name_width = chars as f32 * NAME_CHAR_WIDTH;
    let lines = (name_width / inner_width).ceil().max(1.0);
    if lines > MAX_NAME_LINES {
        return None;
    }
    let label_height = lines * NAME_LINE_HEIGHT + DETAIL_HEIGHT;
    (label_height <= inner_height).then_some((
        name_width.min(inner_width) + 2.0 * LABEL_INSET,
        label_height + 2.0 * LABEL_INSET,
    ))
}

/// Axis-aligned bounds of a `size` rectangle centered at `center`, turned
/// by `rotation`: `(min_x, min_y, max_x, max_y)`.
fn rotated_bounds(center: (f32, f32), size: (f32, f32), rotation: f32) -> (f32, f32, f32, f32) {
    let (sin, cos) = rotation.sin_cos();
    let (half_width, half_height) = (size.0 * 0.5, size.1 * 0.5);
    let extent_x = (half_width * cos).abs() + (half_height * sin).abs();
    let extent_y = (half_width * sin).abs() + (half_height * cos).abs();
    (
        center.0 - extent_x,
        center.1 - extent_y,
        center.0 + extent_x,
        center.1 + extent_y,
    )
}

fn overlaps(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

/// How much room each box offers its name. A box's label is covered when
/// any box painted above it overlaps the label's area.
#[must_use]
pub fn label_rooms(boxes: &[LabelBox]) -> HashMap<String, LabelRoom> {
    boxes
        .iter()
        .map(|candidate| {
            let room = match label_extent(candidate.name_chars, candidate.size.0, candidate.size.1)
            {
                None => LabelRoom::TooSmall,
                Some(extent) => {
                    let label = rotated_bounds(candidate.center, extent, candidate.rotation);
                    let covered = boxes.iter().any(|other| {
                        other.z > candidate.z
                            && overlaps(
                                label,
                                rotated_bounds(other.center, other.size, other.rotation),
                            )
                    });
                    if covered {
                        LabelRoom::Covered
                    } else {
                        LabelRoom::Clear
                    }
                }
            };
            (candidate.id.clone(), room)
        })
        .collect()
}

/// Where a box shows its name. `emphasized` means the box is selected or
/// the device pane is pointing at it; `pinned` means it should name itself
/// even when too small to hold the name (the pane points at it, or it is
/// the box the selection is about).
#[must_use]
pub fn label_mode(room: LabelRoom, emphasized: bool, pinned: bool) -> LabelMode {
    match room {
        LabelRoom::Clear => LabelMode {
            inside: Visibility::Shown,
            callout: Visibility::Hidden,
        },
        LabelRoom::Covered => LabelMode {
            inside: if emphasized {
                Visibility::Shown
            } else {
                Visibility::OnHover
            },
            callout: Visibility::Hidden,
        },
        LabelRoom::TooSmall => LabelMode {
            inside: Visibility::Hidden,
            callout: if pinned {
                Visibility::Shown
            } else {
                Visibility::OnHover
            },
        },
    }
}

/// Tailwind classes that realize a [`Visibility`] on an element inside a
/// `group` box.
#[must_use]
pub fn visibility_class(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Shown => "opacity-100",
        Visibility::OnHover => "opacity-0 group-hover:opacity-100",
        Visibility::Hidden => "opacity-0",
    }
}
