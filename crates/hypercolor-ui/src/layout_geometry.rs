//! Shared layout geometry helpers for default footprints and proportional resizing.
#![allow(
    dead_code,
    reason = "Some helpers are pre-built for upcoming editor features."
)]

use std::f32::consts::{PI, TAU};

use hypercolor_types::spatial::{LedTopology, NormalizedPosition, SpatialLayout, StripDirection};

#[path = "layout_geometry/groups.rs"]
mod groups;

pub use groups::{
    AlignAnchor, AlignAxis, CompoundBounds, align_group, compound_bounding_box, distribute_group,
    group_centroid, mirror_group, pack_group, rotate_group, scale_group, translate_group,
    translate_zones,
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
    for zone in &mut layout.zones {
        zone.size = normalize_zone_size_for_editor(zone.position, zone.size, &zone.topology);
    }
    layout
}

pub fn normalize_zone_size_for_editor(
    position: NormalizedPosition,
    size: NormalizedPosition,
    topology: &LedTopology,
) -> NormalizedPosition {
    match topology {
        LedTopology::Strip { direction, .. } => clamp_strip_size(position, size, *direction),
        LedTopology::Ring { .. } => {
            let side = size.x.min(size.y).max(RESIZE_MIN_SIZE);
            NormalizedPosition::new(side, side)
        }
        _ => size,
    }
}

pub(crate) fn drag_zone_to_position(
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

    let clamped = clamp_zone_center(desired_position, layout.zones[zone_index].size);
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
    let desired_position = NormalizedPosition::new(
        desired_position.x.clamp(0.0, 1.0),
        desired_position.y.clamp(0.0, 1.0),
    );
    let Some(zone_index) = layout.zones.iter().position(|zone| zone.id == zone_id) else {
        return false;
    };

    let clamped = clamp_zone_center(desired_position, layout.zones[zone_index].size);
    if layout.zones[zone_index].position == clamped {
        return false;
    }
    layout.zones[zone_index].position = clamped;
    true
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

pub fn resize_zone_from_handle(
    start_center: NormalizedPosition,
    start_size: NormalizedPosition,
    start_mouse: NormalizedPosition,
    handle: ResizeHandle,
    current_mouse: NormalizedPosition,
    keep_aspect_ratio: bool,
    rotation: f32,
) -> (NormalizedPosition, NormalizedPosition) {
    // Rotate mouse coordinates into zone-local (unrotated) space so that
    // dragging along a rotated edge correctly maps to width/height changes.
    let (local_start, local_current) =
        rotate_mouse_to_local(start_mouse, current_mouse, start_center, rotation);

    if keep_aspect_ratio {
        resize_zone_locked(start_center, start_size, handle, local_current)
    } else {
        resize_zone_unlocked(start_center, start_size, local_start, handle, local_current)
    }
}

/// Rotate two mouse positions from viewport space into zone-local (unrotated)
/// space, pivoting around the zone center.
fn rotate_mouse_to_local(
    start_mouse: NormalizedPosition,
    current_mouse: NormalizedPosition,
    center: NormalizedPosition,
    rotation: f32,
) -> (NormalizedPosition, NormalizedPosition) {
    if rotation.abs() < GRID_EPSILON {
        return (start_mouse, current_mouse);
    }
    let cos_r = (-rotation).cos();
    let sin_r = (-rotation).sin();

    let rotate = |p: NormalizedPosition| {
        let dx = p.x - center.x;
        let dy = p.y - center.y;
        NormalizedPosition::new(
            center.x + dx * cos_r - dy * sin_r,
            center.y + dx * sin_r + dy * cos_r,
        )
    };

    (rotate(start_mouse), rotate(current_mouse))
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

fn resize_zone_unlocked(
    start_center: NormalizedPosition,
    start_size: NormalizedPosition,
    start_mouse: NormalizedPosition,
    handle: ResizeHandle,
    current_mouse: NormalizedPosition,
) -> (NormalizedPosition, NormalizedPosition) {
    let min_size = axis_minimums_for_aspect(zone_aspect_ratio(start_size), RESIZE_MIN_SIZE);
    let start_left = start_center.x - start_size.x * 0.5;
    let start_right = start_center.x + start_size.x * 0.5;
    let start_top = start_center.y - start_size.y * 0.5;
    let start_bottom = start_center.y + start_size.y * 0.5;

    let dx = current_mouse.x - start_mouse.x;
    let dy = current_mouse.y - start_mouse.y;

    let (mut left, mut right, mut top, mut bottom) =
        (start_left, start_right, start_top, start_bottom);

    match handle {
        ResizeHandle::NorthWest => {
            left = (start_left + dx).clamp(0.0, start_right - min_size.x);
            top = (start_top + dy).clamp(0.0, start_bottom - min_size.y);
        }
        ResizeHandle::NorthEast => {
            right = (start_right + dx).clamp(start_left + min_size.x, 1.0);
            top = (start_top + dy).clamp(0.0, start_bottom - min_size.y);
        }
        ResizeHandle::SouthWest => {
            left = (start_left + dx).clamp(0.0, start_right - min_size.x);
            bottom = (start_bottom + dy).clamp(start_top + min_size.y, 1.0);
        }
        ResizeHandle::SouthEast => {
            right = (start_right + dx).clamp(start_left + min_size.x, 1.0);
            bottom = (start_bottom + dy).clamp(start_top + min_size.y, 1.0);
        }
    }

    rect_from_bounds(left, right, top, bottom, min_size)
}

fn resize_zone_locked(
    start_center: NormalizedPosition,
    start_size: NormalizedPosition,
    handle: ResizeHandle,
    current_mouse: NormalizedPosition,
) -> (NormalizedPosition, NormalizedPosition) {
    let start_left = start_center.x - start_size.x * 0.5;
    let start_right = start_center.x + start_size.x * 0.5;
    let start_top = start_center.y - start_size.y * 0.5;
    let start_bottom = start_center.y + start_size.y * 0.5;

    let aspect = (start_size.x / start_size.y.max(GRID_EPSILON)).max(GRID_EPSILON);

    let (anchor_x, anchor_y, horizontal_sign, vertical_sign) = match handle {
        ResizeHandle::NorthWest => (start_right, start_bottom, -1.0, -1.0),
        ResizeHandle::NorthEast => (start_left, start_bottom, 1.0, -1.0),
        ResizeHandle::SouthWest => (start_right, start_top, -1.0, 1.0),
        ResizeHandle::SouthEast => (start_left, start_top, 1.0, 1.0),
    };

    let max_width = if horizontal_sign > 0.0 {
        1.0 - anchor_x
    } else {
        anchor_x
    }
    .max(GRID_EPSILON);
    let max_height = if vertical_sign > 0.0 {
        1.0 - anchor_y
    } else {
        anchor_y
    }
    .max(GRID_EPSILON);

    let max_preserved_width = max_width.min(max_height * aspect).max(GRID_EPSILON);
    let max_preserved_height = (max_preserved_width / aspect).max(GRID_EPSILON);
    let raw_min_preserved = locked_minimum_size(aspect, RESIZE_MIN_SIZE);
    let min_scale = (max_preserved_width / raw_min_preserved.x.max(GRID_EPSILON))
        .min(max_preserved_height / raw_min_preserved.y.max(GRID_EPSILON))
        .min(1.0);
    let min_preserved_size = NormalizedPosition::new(
        raw_min_preserved.x * min_scale,
        raw_min_preserved.y * min_scale,
    );

    // Project the mouse displacement onto the aspect-ratio diagonal for smooth,
    // continuous resizing. This replaces the old two-candidate distance heuristic
    // that caused discrete size jumps when crossing between width-driven and
    // height-driven modes.
    let signed_dx = (current_mouse.x - anchor_x) * horizontal_sign;
    let signed_dy = (current_mouse.y - anchor_y) * vertical_sign;

    // Diagonal direction is (aspect, 1) — the aspect-ratio preserving diagonal.
    // t is the scalar projection onto this diagonal.
    let t = (signed_dx * aspect + signed_dy) / (aspect * aspect + 1.0);

    let width = (t * aspect).clamp(min_preserved_size.x, max_preserved_width);
    let height = (width / aspect).clamp(min_preserved_size.y, max_preserved_height);

    let left = if horizontal_sign > 0.0 {
        anchor_x
    } else {
        anchor_x - width
    };
    let right = if horizontal_sign > 0.0 {
        anchor_x + width
    } else {
        anchor_x
    };
    let top = if vertical_sign > 0.0 {
        anchor_y
    } else {
        anchor_y - height
    };
    let bottom = if vertical_sign > 0.0 {
        anchor_y + height
    } else {
        anchor_y
    };

    rect_from_bounds(left, right, top, bottom, min_preserved_size)
}

fn corner_from_anchor(
    anchor_x: f32,
    anchor_y: f32,
    horizontal_sign: f32,
    vertical_sign: f32,
    width: f32,
    height: f32,
) -> NormalizedPosition {
    NormalizedPosition::new(
        anchor_x + width * horizontal_sign,
        anchor_y + height * vertical_sign,
    )
}

fn rect_from_bounds(
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    min_size: NormalizedPosition,
) -> (NormalizedPosition, NormalizedPosition) {
    (
        NormalizedPosition::new(
            ((left + right) * 0.5).clamp(0.0, 1.0),
            ((top + bottom) * 0.5).clamp(0.0, 1.0),
        ),
        NormalizedPosition::new(
            (right - left).max(min_size.x),
            (bottom - top).max(min_size.y),
        ),
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

fn clamp_strip_size(
    position: NormalizedPosition,
    size: NormalizedPosition,
    direction: StripDirection,
) -> NormalizedPosition {
    let max_width = available_axis_span(position.x);
    let max_height = available_axis_span(position.y);
    let mut width = size.x.clamp(GRID_EPSILON, max_width.max(GRID_EPSILON));
    let mut height = size.y.clamp(GRID_EPSILON, max_height.max(GRID_EPSILON));

    match direction {
        StripDirection::LeftToRight | StripDirection::RightToLeft => {
            let target_height = (width / EDITOR_STRIP_MAX_ASPECT).min(max_height);
            height = height
                .max(target_height)
                .clamp(GRID_EPSILON, max_height.max(GRID_EPSILON));
            if width / height.max(GRID_EPSILON) > EDITOR_STRIP_MAX_ASPECT {
                width = (height * EDITOR_STRIP_MAX_ASPECT)
                    .clamp(GRID_EPSILON, max_width.max(GRID_EPSILON));
            }
        }
        StripDirection::TopToBottom | StripDirection::BottomToTop => {
            let target_width = (height / EDITOR_STRIP_MAX_ASPECT).min(max_width);
            width = width
                .max(target_width)
                .clamp(GRID_EPSILON, max_width.max(GRID_EPSILON));
            if height / width.max(GRID_EPSILON) > EDITOR_STRIP_MAX_ASPECT {
                height = (width * EDITOR_STRIP_MAX_ASPECT)
                    .clamp(GRID_EPSILON, max_height.max(GRID_EPSILON));
            }
        }
    }

    NormalizedPosition::new(width, height)
}

fn clamp_zone_center(position: NormalizedPosition, size: NormalizedPosition) -> NormalizedPosition {
    NormalizedPosition::new(
        position.x.clamp(
            size.x.mul_add(0.5, 0.0).min(1.0),
            (1.0 - size.x * 0.5).max(0.0),
        ),
        position.y.clamp(
            size.y.mul_add(0.5, 0.0).min(1.0),
            (1.0 - size.y * 0.5).max(0.0),
        ),
    )
}

fn available_axis_span(center: f32) -> f32 {
    (center.min(1.0 - center) * 2.0).clamp(GRID_EPSILON, 1.0)
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
