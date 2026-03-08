//! Shared layout geometry helpers for default footprints and proportional resizing.
#![allow(
    dead_code,
    reason = "The geometry layer is being integrated into the editor incrementally."
)]

use std::f32::consts::FRAC_PI_2;

use hypercolor_types::attachment::AttachmentSuggestedZone;
use hypercolor_types::spatial::{
    Corner, LedTopology, NormalizedPosition, Orientation, StripDirection, Winding, ZoneShape,
};

use crate::api::{ZoneSummary, ZoneTopologySummary};

const DEVICE_MIN_SIZE: NormalizedPosition = NormalizedPosition::new(0.08, 0.06);
const DEVICE_MAX_SIZE: NormalizedPosition = NormalizedPosition::new(0.34, 0.24);
const DEVICE_RING_SIZE: NormalizedPosition = NormalizedPosition::new(0.16, 0.16);
const ATTACHMENT_MIN_SIZE: NormalizedPosition = NormalizedPosition::new(0.02, 0.02);
const INPUT_MIN_SIZE: f32 = 0.02;
const RESIZE_MIN_SIZE: f32 = 0.04;
const PROPORTIONAL_DIMENSION_FLOOR: f32 = 0.0001;
const THIN_SHAPE_ASPECT_THRESHOLD: f32 = 3.0;
const GRID_EPSILON: f32 = 0.001;

const BASILISK_V3_GRID: VisualUnits = VisualUnits::new(7.0, 8.0);
const BASILISK_V3_PRO_GRID: VisualUnits = VisualUnits::new(6.0, 7.0);

const BASILISK_V3_POINTS: &[(u32, u32)] = &[
    (3, 5),
    (3, 1),
    (1, 1),
    (0, 2),
    (0, 3),
    (0, 4),
    (2, 6),
    (4, 6),
    (5, 3),
    (6, 2),
    (6, 1),
];

const BASILISK_V3_PRO_POINTS: &[(u32, u32)] = &[
    (3, 4),
    (3, 0),
    (0, 1),
    (0, 2),
    (0, 3),
    (0, 4),
    (1, 5),
    (3, 6),
    (4, 4),
    (5, 3),
    (5, 2),
    (5, 1),
    (5, 0),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SizeAxis {
    Width,
    Height,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeHandle {
    NorthWest,
    NorthEast,
    SouthWest,
    SouthEast,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ZoneVisualDefaults {
    pub topology: LedTopology,
    pub size: NormalizedPosition,
    pub orientation: Option<Orientation>,
    pub shape: Option<ZoneShape>,
    pub shape_preset: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct VisualUnits {
    width: f32,
    height: f32,
}

impl VisualUnits {
    const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    fn aspect_ratio(self) -> f32 {
        (self.width.max(GRID_EPSILON) / self.height.max(GRID_EPSILON)).max(GRID_EPSILON)
    }
}

pub(crate) fn default_zone_visuals(
    device_name: &str,
    zone: Option<&ZoneSummary>,
    total_leds: usize,
) -> ZoneVisualDefaults {
    #[allow(clippy::cast_possible_truncation)]
    let led_count = zone
        .map(|summary| summary.led_count)
        .map(|count| count as u32)
        .unwrap_or(total_leds as u32)
        .max(1);

    if let Some(signal_defaults) = signal_visual_defaults(device_name, led_count) {
        return signal_defaults;
    }

    let zone_name = zone
        .map(|summary| summary.name.to_ascii_lowercase())
        .unwrap_or_default();
    let topology_hint = zone.and_then(|summary| summary.topology_hint.clone());

    if zone_name.contains("strimer") || zone_name.contains("cable") {
        let rows = if led_count >= 48 { 4 } else { 2 };
        let cols = (led_count / rows).max(8);
        return matrix_defaults(rows, cols, Some("strimer-generic"));
    }

    if zone_name.contains("fan") {
        return ring_defaults(led_count.max(12), Some("fan-ring"));
    }

    if zone_name.contains("aio") || zone_name.contains("pump") {
        return ring_defaults(led_count.max(12), Some("aio-pump-ring"));
    }

    if zone_name.contains("radiator") || zone_name.contains("rad") {
        return strip_defaults(
            led_count,
            StripDirection::LeftToRight,
            Some("aio-radiator-strip"),
        );
    }

    match topology_hint {
        Some(ZoneTopologySummary::Strip) => {
            strip_defaults(led_count, StripDirection::LeftToRight, None)
        }
        Some(ZoneTopologySummary::Matrix { rows, cols }) => matrix_defaults(rows, cols, None),
        Some(ZoneTopologySummary::Ring { count }) => ring_defaults(count, None),
        Some(ZoneTopologySummary::Point) => point_defaults(),
        Some(ZoneTopologySummary::Display { width, height, .. }) => {
            matrix_defaults(height, width, Some("lcd-display"))
        }
        Some(ZoneTopologySummary::Custom) | None => {
            if led_count <= 1 {
                point_defaults()
            } else {
                strip_defaults(
                    led_count,
                    StripDirection::LeftToRight,
                    Some("generic-strip"),
                )
            }
        }
    }
}

pub(crate) fn attachment_zone_size(
    suggested: &AttachmentSuggestedZone,
    max_size: NormalizedPosition,
) -> NormalizedPosition {
    let units = topology_visual_units(&suggested.topology);
    fit_visual_units(units, ATTACHMENT_MIN_SIZE, max_size)
}

pub(crate) fn resize_zone_from_handle(
    start_center: NormalizedPosition,
    start_size: NormalizedPosition,
    start_mouse: NormalizedPosition,
    handle: ResizeHandle,
    current_mouse: NormalizedPosition,
    keep_aspect_ratio: bool,
) -> (NormalizedPosition, NormalizedPosition) {
    if keep_aspect_ratio {
        resize_zone_locked(start_center, start_size, handle, current_mouse)
    } else {
        resize_zone_unlocked(start_center, start_size, start_mouse, handle, current_mouse)
    }
}

pub(crate) fn update_zone_size(
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

fn signal_visual_defaults(device_name: &str, led_count: u32) -> Option<ZoneVisualDefaults> {
    let normalized_name = device_name.to_ascii_lowercase();

    if normalized_name.contains("basilisk v3 pro 35k")
        || normalized_name.contains("basilisk v3 pro")
    {
        return sparse_signal_defaults(
            BASILISK_V3_PRO_POINTS,
            BASILISK_V3_PRO_GRID,
            led_count,
            "razer-basilisk-v3-pro",
        );
    }

    if normalized_name.contains("basilisk v3 35k") || normalized_name.contains("basilisk v3") {
        return sparse_signal_defaults(
            BASILISK_V3_POINTS,
            BASILISK_V3_GRID,
            led_count,
            "razer-basilisk-v3",
        );
    }

    None
}

fn sparse_signal_defaults(
    points: &[(u32, u32)],
    grid: VisualUnits,
    led_count: u32,
    shape_preset: &str,
) -> Option<ZoneVisualDefaults> {
    let positions = grid_points(points, grid);
    #[allow(clippy::cast_possible_truncation)]
    if positions.len() as u32 != led_count {
        return None;
    }

    Some(ZoneVisualDefaults {
        topology: LedTopology::Custom { positions },
        size: fit_visual_units(grid, DEVICE_MIN_SIZE, DEVICE_MAX_SIZE),
        orientation: Some(Orientation::Horizontal),
        shape: Some(ZoneShape::Rectangle),
        shape_preset: Some(shape_preset.to_owned()),
    })
}

fn matrix_defaults(rows: u32, cols: u32, shape_preset: Option<&str>) -> ZoneVisualDefaults {
    let grid = VisualUnits::new(cols.max(1) as f32, rows.max(1) as f32);
    let aspect = grid.aspect_ratio();

    ZoneVisualDefaults {
        topology: LedTopology::Matrix {
            width: cols.max(1),
            height: rows.max(1),
            serpentine: false,
            start_corner: Corner::TopLeft,
        },
        size: fit_visual_units(grid, DEVICE_MIN_SIZE, DEVICE_MAX_SIZE),
        orientation: Some(if aspect >= 1.0 {
            Orientation::Horizontal
        } else {
            Orientation::Vertical
        }),
        shape: Some(ZoneShape::Rectangle),
        shape_preset: shape_preset.map(str::to_owned),
    }
}

fn strip_defaults(
    count: u32,
    direction: StripDirection,
    shape_preset: Option<&str>,
) -> ZoneVisualDefaults {
    let topology = LedTopology::Strip {
        count: count.max(1),
        direction,
    };

    ZoneVisualDefaults {
        topology,
        size: fit_visual_units(
            topology_visual_units(&LedTopology::Strip {
                count: count.max(1),
                direction,
            }),
            NormalizedPosition::new(0.10, 0.02),
            NormalizedPosition::new(0.34, 0.12),
        ),
        orientation: Some(match direction {
            StripDirection::LeftToRight | StripDirection::RightToLeft => Orientation::Horizontal,
            StripDirection::TopToBottom | StripDirection::BottomToTop => Orientation::Vertical,
        }),
        shape: Some(ZoneShape::Rectangle),
        shape_preset: shape_preset.map(str::to_owned),
    }
}

fn ring_defaults(count: u32, shape_preset: Option<&str>) -> ZoneVisualDefaults {
    ZoneVisualDefaults {
        topology: LedTopology::Ring {
            count: count.max(1),
            start_angle: -FRAC_PI_2,
            direction: Winding::Clockwise,
        },
        size: DEVICE_RING_SIZE,
        orientation: Some(Orientation::Radial),
        shape: Some(ZoneShape::Ring),
        shape_preset: shape_preset.map(str::to_owned),
    }
}

fn point_defaults() -> ZoneVisualDefaults {
    ZoneVisualDefaults {
        topology: LedTopology::Point,
        size: NormalizedPosition::new(0.08, 0.08),
        orientation: None,
        shape: Some(ZoneShape::Ring),
        shape_preset: None,
    }
}

fn fit_visual_units(
    units: VisualUnits,
    min_size: NormalizedPosition,
    max_size: NormalizedPosition,
) -> NormalizedPosition {
    let aspect = units.aspect_ratio();
    let box_aspect =
        (max_size.x.max(GRID_EPSILON) / max_size.y.max(GRID_EPSILON)).max(GRID_EPSILON);

    let (mut width, mut height) = if aspect >= box_aspect {
        let width = max_size.x.max(GRID_EPSILON);
        (width, width / aspect)
    } else {
        let height = max_size.y.max(GRID_EPSILON);
        (height * aspect, height)
    };

    let min_width_to_keep_ratio = min_size.x.max(min_size.y * aspect);
    let min_height_to_keep_ratio = min_size.y.max(min_size.x / aspect);
    let can_meet_min =
        min_width_to_keep_ratio <= max_size.x && min_height_to_keep_ratio <= max_size.y;

    if can_meet_min && (width < min_size.x || height < min_size.y) {
        let grow =
            (min_size.x / width.max(GRID_EPSILON)).max(min_size.y / height.max(GRID_EPSILON));
        width *= grow;
        height *= grow;
    }

    if width > max_size.x || height > max_size.y {
        let shrink =
            (max_size.x / width.max(GRID_EPSILON)).min(max_size.y / height.max(GRID_EPSILON));
        width *= shrink;
        height *= shrink;
    }

    NormalizedPosition::new(
        width.clamp(GRID_EPSILON, 1.0),
        height.clamp(GRID_EPSILON, 1.0),
    )
}

fn topology_visual_units(topology: &LedTopology) -> VisualUnits {
    match topology {
        LedTopology::Strip { count, direction } => match direction {
            StripDirection::LeftToRight | StripDirection::RightToLeft => {
                VisualUnits::new((*count).max(1) as f32, 1.0)
            }
            StripDirection::TopToBottom | StripDirection::BottomToTop => {
                VisualUnits::new(1.0, (*count).max(1) as f32)
            }
        },
        LedTopology::Matrix { width, height, .. } => {
            VisualUnits::new((*width).max(1) as f32, (*height).max(1) as f32)
        }
        LedTopology::Ring { .. } | LedTopology::ConcentricRings { .. } | LedTopology::Point => {
            VisualUnits::new(1.0, 1.0)
        }
        LedTopology::PerimeterLoop {
            top,
            right,
            bottom,
            left,
            ..
        } => VisualUnits::new(
            (*top).max(*bottom).max(1) as f32,
            (*left).max(*right).max(1) as f32,
        ),
        LedTopology::Custom { positions } => custom_visual_units(positions),
    }
}

fn custom_visual_units(positions: &[NormalizedPosition]) -> VisualUnits {
    let Some(first) = positions.first().copied() else {
        return VisualUnits::new(1.0, 1.0);
    };

    let (mut min_x, mut max_x, mut min_y, mut max_y) = (first.x, first.x, first.y, first.y);
    for position in positions.iter().skip(1) {
        min_x = min_x.min(position.x);
        max_x = max_x.max(position.x);
        min_y = min_y.min(position.y);
        max_y = max_y.max(position.y);
    }

    VisualUnits::new((max_x - min_x).max(0.25), (max_y - min_y).max(0.25))
}

fn grid_points(points: &[(u32, u32)], grid: VisualUnits) -> Vec<NormalizedPosition> {
    points
        .iter()
        .map(|(x, y)| {
            let norm_x = if grid.width <= 1.0 {
                0.5
            } else {
                *x as f32 / (grid.width - 1.0)
            };
            let norm_y = if grid.height <= 1.0 {
                0.5
            } else {
                *y as f32 / (grid.height - 1.0)
            };
            NormalizedPosition::new(norm_x, norm_y)
        })
        .collect()
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

    let candidate_width_from_x = if horizontal_sign > 0.0 {
        current_mouse.x - anchor_x
    } else {
        anchor_x - current_mouse.x
    }
    .abs();
    let candidate_height_from_y = if vertical_sign > 0.0 {
        current_mouse.y - anchor_y
    } else {
        anchor_y - current_mouse.y
    }
    .abs();

    let option_width_x = candidate_width_from_x.clamp(min_preserved_size.x, max_preserved_width);
    let option_height_x = option_width_x / aspect;
    let option_height_y = candidate_height_from_y.clamp(min_preserved_size.y, max_preserved_height);
    let option_width_y = option_height_y * aspect;

    let handle_from_width = corner_from_anchor(
        anchor_x,
        anchor_y,
        horizontal_sign,
        vertical_sign,
        option_width_x,
        option_height_x,
    );
    let handle_from_height = corner_from_anchor(
        anchor_x,
        anchor_y,
        horizontal_sign,
        vertical_sign,
        option_width_y,
        option_height_y,
    );

    let (width, height) = if distance_sq(handle_from_width, current_mouse)
        <= distance_sq(handle_from_height, current_mouse)
    {
        (option_width_x, option_height_x)
    } else {
        (option_width_y, option_height_y)
    };

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

fn distance_sq(left: NormalizedPosition, right: NormalizedPosition) -> f32 {
    let dx = left.x - right.x;
    let dy = left.y - right.y;
    dx * dx + dy * dy
}
