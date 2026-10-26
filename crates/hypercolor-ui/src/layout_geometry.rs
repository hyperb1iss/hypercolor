//! Shared layout geometry helpers for default footprints and proportional resizing.
#![allow(
    dead_code,
    reason = "Some helpers are pre-built for upcoming editor features."
)]

use std::f32::consts::{PI, TAU};

use hypercolor_types::spatial::{
    LedTopology, NormalizedPosition, Output, SpatialLayout, StripDirection, ZoneShape,
};

#[path = "layout_geometry/compounds.rs"]
mod compounds;

pub use compounds::{
    AlignAnchor, AlignAxis, CompoundBounds, align_compound, compound_bounding_box,
    compound_centroid, distribute_compound, mirror_compound, pack_compound, rotate_compound,
    scale_compound, translate_compound, translate_zones,
};
#[path = "layout_geometry/defaults.rs"]
mod defaults;

pub use defaults::{
    SeededAttachmentLayout, SeededDeviceLayout, ZoneVisualDefaults, attachment_zone_shape,
    attachment_zone_size, default_zone_visuals, seeded_attachment_layout, seeded_device_layout,
};

const INPUT_MIN_SIZE: f32 = 0.02;
const RESIZE_MIN_SIZE: f32 = 0.04;
const PROPORTIONAL_DIMENSION_FLOOR: f32 = 0.0001;
const THIN_SHAPE_ASPECT_THRESHOLD: f32 = 3.0;
const GRID_EPSILON: f32 = 0.001;
const EDITOR_STRIP_MAX_ASPECT: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SizeAxis {
    Width,
    Height,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeHandle {
    NorthWest,
    NorthEast,
    SouthWest,
    SouthEast,
}

pub(crate) fn normalize_layout_for_editor(mut layout: SpatialLayout) -> SpatialLayout {
    let aspect = canvas_pixel_aspect(layout.canvas_width, layout.canvas_height);
    for zone in &mut layout.zones {
        zone.size = normalize_zone_size_for_editor(
            zone.position,
            zone.size,
            &zone.topology,
            zone.shape.as_ref(),
            aspect,
        );
    }
    layout
}

/// Whether a zone renders and samples as a circle. The explicit shape
/// wins when present; a bare Ring topology is circular by default. This
/// is THE circularity predicate: locked resize, release normalization,
/// and the canvas CSS all key off it so they can never disagree.
pub fn is_circular_zone(shape: Option<&ZoneShape>, topology: &LedTopology) -> bool {
    match shape {
        Some(ZoneShape::Ring | ZoneShape::Arc { .. }) => true,
        Some(_) => false,
        None => matches!(topology, LedTopology::Ring { .. }),
    }
}

/// Pixel aspect (width / height) of a canvas extent.
pub fn canvas_pixel_aspect(canvas_width: u32, canvas_height: u32) -> f32 {
    #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
    let ratio = canvas_width.max(1) as f32 / canvas_height.max(1) as f32;
    ratio.max(GRID_EPSILON)
}

/// The size a box keeps in the editor: circles square in pixels, strips
/// thick enough to see. A box's size never depends on where it sits; a
/// box near an edge stays on the canvas through its position, which
/// [`drawn_extent`] measures as drawn, rotation included.
pub fn normalize_zone_size_for_editor(
    _position: NormalizedPosition,
    size: NormalizedPosition,
    topology: &LedTopology,
    shape: Option<&ZoneShape>,
    canvas_aspect: f32,
) -> NormalizedPosition {
    if is_circular_zone(shape, topology) {
        return pixel_square_size(size, canvas_aspect);
    }
    match topology {
        LedTopology::Strip { direction, .. } => clamp_strip_size(size, *direction),
        _ => size,
    }
}

/// Square in canvas *pixels*, matching both the CSS (`aspect-ratio: 1`)
/// and the sampled LED circle, which lives in the zone's pixel rect. A
/// normalized square on a non-square canvas is an ellipse in both.
fn pixel_square_size(size: NormalizedPosition, canvas_aspect: f32) -> NormalizedPosition {
    let aspect = canvas_aspect.max(GRID_EPSILON);
    let mut width = size.x.min(size.y / aspect).max(RESIZE_MIN_SIZE);
    let mut height = width * aspect;
    if height > 1.0 {
        height = 1.0;
        width = height / aspect;
    }
    NormalizedPosition::new(width, height)
}

/// Half the width and height of the box `output` draws, in its own frame
/// and in canvas pixel proportions (x spans the canvas aspect, y spans 1),
/// scale included. A circle draws square in pixels, sized by its width.
#[must_use]
pub fn drawn_half_size(output: &Output, aspect: f32) -> (f32, f32) {
    let scale = output.scale.max(0.0);
    let half_width = output.size.x * aspect * scale * 0.5;
    let half_height = if is_circular_zone(output.shape.as_ref(), &output.topology) {
        half_width
    } else {
        output.size.y * scale * 0.5
    };
    (half_width, half_height)
}

/// Width and height of the axis-aligned bounds of the box `output` draws
/// on a canvas of pixel aspect `aspect`, in normalized canvas units. The
/// box turns in pixels, so on a canvas that is not square a quarter turn
/// trades its pixel width for its pixel height, not its normalized
/// width for its normalized height. A circle's bounds ignore rotation.
#[must_use]
pub fn drawn_extent(output: &Output, aspect: f32) -> NormalizedPosition {
    let aspect = aspect.max(GRID_EPSILON);
    let (half_width, half_height) = drawn_half_size(output, aspect);
    let (extent_x, extent_y) = if is_circular_zone(output.shape.as_ref(), &output.topology) {
        (half_width, half_width)
    } else {
        let (sin, cos) = output.rotation.sin_cos();
        (
            (half_width * cos).abs() + (half_height * sin).abs(),
            (half_width * sin).abs() + (half_height * cos).abs(),
        )
    };
    NormalizedPosition::new(2.0 * extent_x / aspect, 2.0 * extent_y)
}

/// [`drawn_extent`] on `layout`'s canvas.
pub(crate) fn layout_extent(layout: &SpatialLayout, output: &Output) -> NormalizedPosition {
    drawn_extent(
        output,
        canvas_pixel_aspect(layout.canvas_width, layout.canvas_height),
    )
}

/// Move one box's center toward `desired_position`, stopping where its
/// drawn bounds meet the canvas edge. Reports whether it moved.
pub fn drag_zone_to_position(
    layout: &mut SpatialLayout,
    zone_id: &str,
    desired_position: NormalizedPosition,
) -> bool {
    let desired_position = NormalizedPosition::new(
        desired_position.x.clamp(0.0, 1.0),
        desired_position.y.clamp(0.0, 1.0),
    );
    let Some(zone_index) = layout.zones.iter().position(|zone| zone.id == zone_id) else {
        return false;
    };

    let extent = layout_extent(layout, &layout.zones[zone_index]);
    let clamped = clamp_zone_center(desired_position, extent);
    if layout.zones[zone_index].position == clamped {
        return false;
    }
    layout.zones[zone_index].position = clamped;
    true
}

pub(crate) fn set_zone_position(
    layout: &mut SpatialLayout,
    zone_id: &str,
    desired_position: NormalizedPosition,
) -> bool {
    drag_zone_to_position(layout, zone_id, desired_position)
}

pub(crate) fn zone_transform_anchor(
    layout: &SpatialLayout,
    zone_id: &str,
) -> Option<NormalizedPosition> {
    layout
        .zones
        .iter()
        .find(|zone| zone.id == zone_id)
        .map(|zone| zone.position)
}

pub fn set_zone_rotation(layout: &mut SpatialLayout, zone_id: &str, rotation: f32) -> bool {
    let Some(zone_index) = layout.zones.iter().position(|zone| zone.id == zone_id) else {
        return false;
    };

    let current_rotation = layout.zones[zone_index].rotation;
    let delta = normalize_rotation_delta(rotation - current_rotation);
    if delta.abs() <= GRID_EPSILON {
        return false;
    }

    layout.zones[zone_index].rotation = normalize_rotation(rotation);
    true
}

/// A box as it stood when one of its resize handles was pressed, and
/// where the press landed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeStart {
    pub center: NormalizedPosition,
    pub size: NormalizedPosition,
    pub rotation: f32,
    pub scale: f32,
    pub mouse: NormalizedPosition,
    pub handle: ResizeHandle,
}

/// The center and size a resize lands on once the pointer reaches
/// `current_mouse`, on a canvas of pixel aspect `canvas_aspect`.
///
/// The solve runs on the box as drawn, in canvas pixel proportions: the
/// pointer turns into the box's own frame, the corner opposite the handle
/// stays put on screen, and the box grows only until its drawn corners
/// meet the canvas edge. A box that already overhangs an edge can still
/// shrink, but never grows further out.
#[must_use]
pub fn resize_zone_from_handle(
    start: ResizeStart,
    current_mouse: NormalizedPosition,
    keep_aspect_ratio: bool,
    canvas_aspect: f32,
) -> (NormalizedPosition, NormalizedPosition) {
    let aspect = canvas_aspect.max(GRID_EPSILON);
    let scale = start.scale.max(GRID_EPSILON);
    let to_drawn = NormalizedPosition::new(aspect * scale, scale);
    let drawn_start = NormalizedPosition::new(start.size.x * to_drawn.x, start.size.y * to_drawn.y);
    let (sin, cos) = start.rotation.sin_cos();
    let turn = |local: (f32, f32)| (local.0 * cos - local.1 * sin, local.0 * sin + local.1 * cos);
    let (sign_x, sign_y) = handle_signs(start.handle);

    // The pointer's travel, turned into the box's own frame.
    let travel = (
        (current_mouse.x - start.mouse.x) * aspect,
        current_mouse.y - start.mouse.y,
    );
    let local_travel = (
        travel.0 * cos + travel.1 * sin,
        -travel.0 * sin + travel.1 * cos,
    );
    let desired = (
        drawn_start.x + sign_x * local_travel.0,
        drawn_start.y + sign_y * local_travel.1,
    );

    let anchor_offset = turn((-sign_x * drawn_start.x * 0.5, -sign_y * drawn_start.y * 0.5));
    let corners = DrawnCorners {
        anchor: (
            start.center.x * aspect + anchor_offset.0,
            start.center.y + anchor_offset.1,
        ),
        along_width: turn((sign_x, 0.0)),
        along_height: turn((0.0, sign_y)),
        canvas: (aspect, 1.0),
    };

    let (width, height) = if keep_aspect_ratio {
        let floor = locked_minimum_size(zone_aspect_ratio(start.size), RESIZE_MIN_SIZE);
        let floor = NormalizedPosition::new(
            floor.x.min(start.size.x) * to_drawn.x,
            floor.y.min(start.size.y) * to_drawn.y,
        );
        resize_locked(drawn_start, desired, floor, &corners)
    } else {
        // A box already smaller than the resize floor (attachment seeds
        // start at 2%) keeps its own size as the floor.
        let floor = axis_minimums_for_aspect(zone_aspect_ratio(start.size), RESIZE_MIN_SIZE);
        let floor = NormalizedPosition::new(
            floor.x.min(start.size.x).max(GRID_EPSILON) * to_drawn.x,
            floor.y.min(start.size.y).max(GRID_EPSILON) * to_drawn.y,
        );
        resize_unlocked(drawn_start, desired, floor, &corners)
    };

    let center = (
        corners.anchor.0 + (corners.along_width.0 * width + corners.along_height.0 * height) * 0.5,
        corners.anchor.1 + (corners.along_width.1 * width + corners.along_height.1 * height) * 0.5,
    );
    (
        NormalizedPosition::new(
            (center.0 / aspect).clamp(0.0, 1.0),
            center.1.clamp(0.0, 1.0),
        ),
        NormalizedPosition::new(width / to_drawn.x, height / to_drawn.y),
    )
}

/// Which way a handle grows the box along its own width and height.
fn handle_signs(handle: ResizeHandle) -> (f32, f32) {
    match handle {
        ResizeHandle::NorthWest => (-1.0, -1.0),
        ResizeHandle::NorthEast => (1.0, -1.0),
        ResizeHandle::SouthWest => (-1.0, 1.0),
        ResizeHandle::SouthEast => (1.0, 1.0),
    }
}

/// A box being resized, as drawn in canvas pixel proportions: the corner
/// that stays put, and the screen directions its width and height grow
/// in. Every corner is `anchor + w * along_width + h * along_height` with
/// `w` either 0 or the width and `h` either 0 or the height.
struct DrawnCorners {
    anchor: (f32, f32),
    along_width: (f32, f32),
    along_height: (f32, f32),
    canvas: (f32, f32),
}

impl DrawnCorners {
    /// The largest `t` that keeps `base + t * direction` on the canvas for
    /// every base. A direction that leaves a coordinate alone does not
    /// bound it.
    fn reach(&self, bases: &[(f32, f32)], direction: (f32, f32)) -> f32 {
        let axis = |base: f32, step: f32, limit: f32| {
            if step > GRID_EPSILON {
                (limit - base) / step
            } else if step < -GRID_EPSILON {
                -base / step
            } else {
                f32::INFINITY
            }
        };
        bases
            .iter()
            .map(|base| {
                axis(base.0, direction.0, self.canvas.0).min(axis(
                    base.1,
                    direction.1,
                    self.canvas.1,
                ))
            })
            .fold(f32::INFINITY, f32::min)
    }

    /// The widest the box can draw at `height` with every corner on the
    /// canvas.
    fn max_width(&self, height: f32) -> f32 {
        let raised = (
            self.anchor.0 + self.along_height.0 * height,
            self.anchor.1 + self.along_height.1 * height,
        );
        self.reach(&[self.anchor, raised], self.along_width)
    }

    /// The tallest the box can draw at `width` with every corner on the
    /// canvas.
    fn max_height(&self, width: f32) -> f32 {
        let widened = (
            self.anchor.0 + self.along_width.0 * width,
            self.anchor.1 + self.along_width.1 * width,
        );
        self.reach(&[self.anchor, widened], self.along_height)
    }
}

/// Keep `desired` within `cap`, except that a box which already overhung
/// the canvas may still shrink toward, but not past, where it started.
fn within_canvas(desired: f32, cap: f32, start: f32) -> f32 {
    desired.min(cap.max(desired.min(start)))
}

fn resize_unlocked(
    start: NormalizedPosition,
    desired: (f32, f32),
    floor: NormalizedPosition,
    corners: &DrawnCorners,
) -> (f32, f32) {
    // Every corner moves away from the anchor as the box grows, so the
    // sizes that fit form a region closed toward the anchor: settle the
    // width against the smaller height, then the height against that
    // width, then the width once more against the final height.
    let mut width = within_canvas(
        desired.0,
        corners.max_width(desired.1.min(start.y).max(0.0)),
        start.x,
    );
    let height = within_canvas(desired.1, corners.max_height(width.max(0.0)), start.y);
    width = within_canvas(width, corners.max_width(height.max(0.0)), start.x);
    (width.max(floor.x), height.max(floor.y))
}

fn resize_locked(
    start: NormalizedPosition,
    desired: (f32, f32),
    floor: NormalizedPosition,
    corners: &DrawnCorners,
) -> (f32, f32) {
    let aspect = (start.x / start.y.max(GRID_EPSILON)).max(GRID_EPSILON);
    // Project the pointer onto the aspect-preserving diagonal so resizing
    // stays smooth instead of jumping between width-led and height-led.
    let height = (desired.0 * aspect + desired.1) / (aspect * aspect + 1.0);
    let widthwise = (
        corners.along_width.0 * aspect,
        corners.along_width.1 * aspect,
    );
    let diagonal = (
        widthwise.0 + corners.along_height.0,
        widthwise.1 + corners.along_height.1,
    );
    let reach = corners
        .reach(&[corners.anchor], widthwise)
        .min(corners.reach(&[corners.anchor], corners.along_height))
        .min(corners.reach(&[corners.anchor], diagonal));
    let height = within_canvas(height, reach, start.y).max(floor.y);
    (height * aspect, height)
}

pub fn update_zone_size(
    current_size: NormalizedPosition,
    axis: SizeAxis,
    raw_value: f32,
    keep_aspect_ratio: bool,
) -> NormalizedPosition {
    let aspect = zone_aspect_ratio(current_size);
    let min_axis_size = axis_minimums_for_aspect(aspect, INPUT_MIN_SIZE);
    let axis_min = match axis {
        SizeAxis::Width => min_axis_size.x,
        SizeAxis::Height => min_axis_size.y,
    };
    let value = raw_value.clamp(axis_min.min(1.0), 1.0);

    if !keep_aspect_ratio || current_size.x <= GRID_EPSILON || current_size.y <= GRID_EPSILON {
        return match axis {
            SizeAxis::Width => NormalizedPosition::new(value.max(min_axis_size.x), current_size.y),
            SizeAxis::Height => NormalizedPosition::new(current_size.x, value.max(min_axis_size.y)),
        };
    }

    let (mut width, mut height) = match axis {
        SizeAxis::Width => (value, value / aspect),
        SizeAxis::Height => (value * aspect, value),
    };
    let min_locked_size = locked_minimum_size(aspect, INPUT_MIN_SIZE);

    if width > 1.0 || height > 1.0 {
        let shrink = (1.0 / width.max(GRID_EPSILON)).min(1.0 / height.max(GRID_EPSILON));
        width *= shrink;
        height *= shrink;
    }

    if width < min_locked_size.x || height < min_locked_size.y {
        let grow = (min_locked_size.x / width.max(GRID_EPSILON))
            .max(min_locked_size.y / height.max(GRID_EPSILON));
        width = (width * grow).min(1.0);
        height = (height * grow).min(1.0);
        if width > 1.0 || height > 1.0 {
            let shrink = (1.0 / width.max(GRID_EPSILON)).min(1.0 / height.max(GRID_EPSILON));
            width *= shrink;
            height *= shrink;
        }
    }

    NormalizedPosition::new(
        width.clamp(min_locked_size.x.min(1.0), 1.0),
        height.clamp(min_locked_size.y.min(1.0), 1.0),
    )
}

fn zone_aspect_ratio(size: NormalizedPosition) -> f32 {
    (size.x / size.y.max(GRID_EPSILON)).max(GRID_EPSILON)
}

fn axis_minimums_for_aspect(aspect: f32, base_min: f32) -> NormalizedPosition {
    let thin_min = proportional_minor_axis_minimum(aspect, base_min);
    if aspect >= THIN_SHAPE_ASPECT_THRESHOLD {
        NormalizedPosition::new(base_min, thin_min)
    } else if aspect <= (1.0 / THIN_SHAPE_ASPECT_THRESHOLD) {
        NormalizedPosition::new(thin_min, base_min)
    } else {
        NormalizedPosition::new(base_min, base_min)
    }
}

fn proportional_minor_axis_minimum(aspect: f32, base_min: f32) -> f32 {
    let long_axis_aspect = aspect.max(1.0 / aspect.max(GRID_EPSILON));
    (base_min / long_axis_aspect)
        .max(PROPORTIONAL_DIMENSION_FLOOR)
        .min(base_min)
}

fn locked_minimum_size(aspect: f32, base_min: f32) -> NormalizedPosition {
    let axis_mins = axis_minimums_for_aspect(aspect, base_min);
    if aspect >= 1.0 {
        let width = axis_mins.x.max(axis_mins.y * aspect);
        NormalizedPosition::new(width, width / aspect)
    } else {
        let height = axis_mins.y.max(axis_mins.x / aspect.max(GRID_EPSILON));
        NormalizedPosition::new(height * aspect, height)
    }
}

fn clamp_strip_size(size: NormalizedPosition, direction: StripDirection) -> NormalizedPosition {
    let mut width = size.x.clamp(GRID_EPSILON, 1.0);
    let mut height = size.y.clamp(GRID_EPSILON, 1.0);

    match direction {
        StripDirection::LeftToRight | StripDirection::RightToLeft => {
            let target_height = (width / EDITOR_STRIP_MAX_ASPECT).min(1.0);
            height = height.max(target_height).clamp(GRID_EPSILON, 1.0);
            if width / height.max(GRID_EPSILON) > EDITOR_STRIP_MAX_ASPECT {
                width = (height * EDITOR_STRIP_MAX_ASPECT).clamp(GRID_EPSILON, 1.0);
            }
        }
        StripDirection::TopToBottom | StripDirection::BottomToTop => {
            let target_width = (height / EDITOR_STRIP_MAX_ASPECT).min(1.0);
            width = width.max(target_width).clamp(GRID_EPSILON, 1.0);
            if height / width.max(GRID_EPSILON) > EDITOR_STRIP_MAX_ASPECT {
                height = (width * EDITOR_STRIP_MAX_ASPECT).clamp(GRID_EPSILON, 1.0);
            }
        }
    }

    NormalizedPosition::new(width, height)
}

/// The nearest center to `position` that keeps a box whose drawn bounds
/// span `extent` (see [`drawn_extent`]) on the canvas.
fn clamp_zone_center(
    position: NormalizedPosition,
    extent: NormalizedPosition,
) -> NormalizedPosition {
    let (min_x, max_x) = zone_center_range(extent.x);
    let (min_y, max_y) = zone_center_range(extent.y);
    NormalizedPosition::new(
        position.x.clamp(min_x, max_x),
        position.y.clamp(min_y, max_y),
    )
}

/// The centers that keep a box of `extent` inside the canvas on one axis.
/// A box wider than the canvas can only be centered, so the range collapses
/// to the midpoint instead of inverting.
pub(crate) fn zone_center_range(extent: f32) -> (f32, f32) {
    let half = extent.max(0.0) * 0.5;
    if half >= 0.5 {
        (0.5, 0.5)
    } else {
        (half, 1.0 - half)
    }
}

fn distance_sq(left: NormalizedPosition, right: NormalizedPosition) -> f32 {
    let dx = left.x - right.x;
    let dy = left.y - right.y;
    dx * dx + dy * dy
}

fn normalize_rotation(rotation: f32) -> f32 {
    rotation.rem_euclid(TAU)
}

fn normalize_rotation_delta(delta: f32) -> f32 {
    (delta + PI).rem_euclid(TAU) - PI
}
