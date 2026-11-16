//! When a box on the canvas shows its name.
//!
//! A dense rig turns into a wall of text when every box names itself, so a
//! box names itself at rest only when its name fits inside it and nothing
//! painted above covers the name. Otherwise the name waits for hover or
//! selection, and a box too small to hold it names itself in a callout
//! beside the box instead.
//!
//! A callout reads level whatever way its box is turned: it sits over the
//! box's bounds on screen, under them when the canvas clips above, and
//! slides sideways to stay inside the canvas. Only the one box something
//! points at pins its callout. When the device pane points at several
//! boxes at once and some cannot hold their names, a single caption over
//! all of them names the group.

use std::collections::HashMap;

/// Estimated advance of one character of a box name, in pixels.
const NAME_CHAR_WIDTH: f32 = 6.4;
/// One line of a box name, in pixels.
const NAME_LINE_HEIGHT: f32 = 14.0;
/// The detail line under the name (LED count or resolution), in pixels.
const DETAIL_HEIGHT: f32 = 15.0;
/// Inset between a box edge and its label, in pixels.
const LABEL_INSET: f32 = 4.0;
/// A name wraps onto at most this many lines.
const MAX_NAME_LINES: f32 = 2.0;
/// Estimated advance of one character of a callout, in pixels.
const CALLOUT_CHAR_WIDTH: f32 = 5.4;
/// A callout's side padding, both sides together, in pixels.
const CALLOUT_PADDING: f32 = 16.0;
/// A callout's height, in pixels.
const CALLOUT_HEIGHT: f32 = 18.0;
/// Gap between the bounds a callout names and the callout, in pixels.
const CALLOUT_GAP: f32 = 5.0;
/// Margin a callout keeps from the canvas edge, in pixels.
const CALLOUT_MARGIN: f32 = 4.0;

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

/// Where a level callout of `chars` characters sits for bounds centered
/// at `center` that reach `reach` each way, on a `viewport` of that many
/// pixels: the offset from `center` to the callout's own center.
///
/// The callout sits centered above the bounds. When the canvas clips it
/// there, it sits below; when neither side has room, it stays inside the
/// canvas edge nearest to above. Sideways it slides only as far as it
/// must to keep its whole width on the canvas.
#[must_use]
pub fn callout_offset(
    center: (f32, f32),
    reach: (f32, f32),
    chars: usize,
    viewport: (f32, f32),
) -> (f32, f32) {
    #[allow(clippy::cast_precision_loss, reason = "names are short")]
    let half_width = (chars as f32 * CALLOUT_CHAR_WIDTH + CALLOUT_PADDING) * 0.5;
    let half_height = CALLOUT_HEIGHT * 0.5;
    let rise = reach.1 + CALLOUT_GAP + half_height;
    let top = CALLOUT_MARGIN + half_height;
    let bottom = viewport.1 - CALLOUT_MARGIN - half_height;
    let above = center.1 - rise;
    let below = center.1 + rise;
    let y = if above >= top || below > bottom {
        // `max` last: a canvas shorter than a callout keeps the top edge.
        above.min(bottom).max(top)
    } else {
        below
    };
    let left = CALLOUT_MARGIN + half_width;
    let right = viewport.0 - CALLOUT_MARGIN - half_width;
    let x = if left <= right {
        center.0.clamp(left, right)
    } else {
        viewport.0 * 0.5
    };
    (x - center.0, y - center.1)
}

/// The bounds that hold every one of `boxes`, each given as a center and
/// its reach each way: the center and reach of their union. `None` when
/// there are no boxes.
#[must_use]
pub fn union_bounds(
    boxes: impl IntoIterator<Item = ((f32, f32), (f32, f32))>,
) -> Option<((f32, f32), (f32, f32))> {
    boxes
        .into_iter()
        .map(|(center, reach)| {
            (
                center.0 - reach.0,
                center.1 - reach.1,
                center.0 + reach.0,
                center.1 + reach.1,
            )
        })
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
        .map(|(min_x, min_y, max_x, max_y)| {
            (
                ((min_x + max_x) * 0.5, (min_y + max_y) * 0.5),
                ((max_x - min_x) * 0.5, (max_y - min_y) * 0.5),
            )
        })
}

/// The caption over a group of `outputs` boxes the device pane points at:
/// the device's name when one device owns them all, then how many.
#[must_use]
pub fn group_caption(device: Option<&str>, outputs: usize) -> String {
    match device {
        Some(device) => format!("{device} · {outputs} outputs"),
        None => format!("{outputs} outputs"),
    }
}

/// The line under a box's name: a display's resolution, since its pixels
/// are not LEDs, or the output's LED count.
#[must_use]
pub fn detail_line(led_count: u32, display_resolution: Option<(u32, u32)>) -> String {
    match display_resolution {
        Some((width, height)) => format!("{width} \u{d7} {height}"),
        None if led_count == 1 => "1 LED".to_owned(),
        None => format!("{led_count} LEDs"),
    }
}

/// The label a box of `width` by `height` pixels needs for a name of
/// `chars` characters, or `None` when the box cannot hold the name on two
/// lines above its detail line.
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
/// something points at it; `pinned` means it should name itself even when
/// too small to hold the name (it is the one box pointed at, or the box
/// the selection is about).
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
