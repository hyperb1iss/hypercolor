//! Shared layout geometry helpers for default footprints and proportional resizing.
#![allow(
    dead_code,
    reason = "The geometry layer is being integrated into the editor incrementally."
)]

use std::f32::consts::FRAC_PI_2;

use hypercolor_types::attachment::{AttachmentCategory, AttachmentSuggestedZone};
use hypercolor_types::spatial::{
    Corner, DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, Orientation,
    SamplingMode, SpatialLayout, StripDirection, Winding, ZoneShape,
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
const EDITOR_STRIP_MAX_ASPECT: f32 = 8.0;

const BASILISK_V3_GRID: VisualUnits = VisualUnits::new(7.0, 8.0);
const BASILISK_V3_PRO_GRID: VisualUnits = VisualUnits::new(6.0, 7.0);
const PUSH2_FOOTPRINT_GRID: VisualUnits = VisualUnits::new(1393.0, 1123.0);
const PUSH2_FOOTPRINT_MIN_SIZE: NormalizedPosition = NormalizedPosition::new(0.42, 0.36);
const PUSH2_FOOTPRINT_MAX_SIZE: NormalizedPosition = NormalizedPosition::new(0.72, 0.82);
const PUSH2_FOOTPRINT_CENTER: NormalizedPosition = NormalizedPosition::new(0.5, 0.5);
const PUSH2_GROUP_COLOR: &str = "#80ffea";
const PUSH2_TRANSPORT_RECT: FootprintRect = FootprintRect::new(16.0, 74.0, 130.0, 959.0);
const PUSH2_WHITE_BUTTONS_RECT: FootprintRect = FootprintRect::new(16.0, 128.0, 1334.0, 903.0);

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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SeededDeviceLayout {
    pub group_id: String,
    pub group_name: String,
    pub group_color: String,
    pub zones: Vec<DeviceZone>,
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
    canvas_width: u32,
    canvas_height: u32,
) -> ZoneVisualDefaults {
    let canvas_aspect = canvas_aspect_ratio(canvas_width, canvas_height);

    #[allow(clippy::cast_possible_truncation)]
    let led_count = zone
        .map(|summary| summary.led_count)
        .map(|count| count as u32)
        .unwrap_or(total_leds as u32)
        .max(1);

    if let Some(signal_defaults) = signal_visual_defaults(device_name, led_count, canvas_aspect) {
        return signal_defaults;
    }

    let zone_name = zone
        .map(|summary| summary.name.to_ascii_lowercase())
        .unwrap_or_default();
    let topology_hint = zone.and_then(|summary| summary.topology_hint.clone());

    if zone_name.contains("strimer") || zone_name.contains("cable") {
        let rows = if led_count >= 48 { 4 } else { 2 };
        let cols = (led_count / rows).max(8);
        return matrix_defaults(rows, cols, Some("strimer-generic"), canvas_aspect);
    }

    if zone_name.contains("fan") {
        return ring_defaults(led_count.max(12), Some("fan-ring"), canvas_aspect);
    }

    if zone_name.contains("aio") || zone_name.contains("pump") {
        return ring_defaults(led_count.max(12), Some("aio-pump-ring"), canvas_aspect);
    }

    if zone_name.contains("radiator") || zone_name.contains("rad") {
        return strip_defaults(
            led_count,
            StripDirection::LeftToRight,
            Some("aio-radiator-strip"),
            canvas_aspect,
        );
    }

    match topology_hint {
        Some(ZoneTopologySummary::Strip) => {
            strip_defaults(led_count, StripDirection::LeftToRight, None, canvas_aspect)
        }
        Some(ZoneTopologySummary::Matrix { rows, cols }) => {
            matrix_defaults(rows, cols, None, canvas_aspect)
        }
        Some(ZoneTopologySummary::Ring { count }) => ring_defaults(count, None, canvas_aspect),
        Some(ZoneTopologySummary::Point) => point_defaults(canvas_aspect),
        Some(ZoneTopologySummary::Display { width, height, .. }) => {
            matrix_defaults(height, width, Some("lcd-display"), canvas_aspect)
        }
        Some(ZoneTopologySummary::Custom) | None => {
            if led_count <= 1 {
                point_defaults(canvas_aspect)
            } else {
                strip_defaults(
                    led_count,
                    StripDirection::LeftToRight,
                    Some("generic-strip"),
                    canvas_aspect,
                )
            }
        }
    }
}

pub(crate) fn seeded_device_layout(
    device_id: &str,
    device_name: &str,
    zones: &[ZoneSummary],
    canvas_width: u32,
    canvas_height: u32,
    display_order_start: i32,
) -> Option<SeededDeviceLayout> {
    if !looks_like_push2(device_name, zones) {
        return None;
    }

    let canvas_aspect = canvas_aspect_ratio(canvas_width, canvas_height);
    let footprint_size = fit_visual_units_for_canvas(
        PUSH2_FOOTPRINT_GRID,
        PUSH2_FOOTPRINT_MIN_SIZE,
        PUSH2_FOOTPRINT_MAX_SIZE,
        canvas_aspect,
    );
    let group_id = format!("device_{}", sanitize_layout_identifier(device_id));
    let mut zones_by_name = zones
        .iter()
        .map(|zone| (zone.name.as_str(), zone))
        .collect::<std::collections::HashMap<_, _>>();
    let mut seeded_zones = Vec::new();

    for (offset, zone_name) in push2_zone_order().iter().enumerate() {
        let Some(zone_summary) = zones_by_name.remove(zone_name) else {
            continue;
        };
        let topology = push2_zone_topology(zone_summary);
        let (position, size) = push2_zone_geometry(zone_name, footprint_size);
        let shape = Some(ZoneShape::Rectangle);
        let orientation = match &topology {
            LedTopology::Strip { direction, .. } => Some(match direction {
                StripDirection::LeftToRight | StripDirection::RightToLeft => {
                    Orientation::Horizontal
                }
                StripDirection::TopToBottom | StripDirection::BottomToTop => Orientation::Vertical,
            }),
            LedTopology::Matrix { width, height, .. } => Some(if width >= height {
                Orientation::Horizontal
            } else {
                Orientation::Vertical
            }),
            LedTopology::Custom { .. } => Some(Orientation::Horizontal),
            LedTopology::Ring { .. } | LedTopology::ConcentricRings { .. } => {
                Some(Orientation::Radial)
            }
            LedTopology::PerimeterLoop { .. } | LedTopology::Point => None,
        };

        seeded_zones.push(DeviceZone {
            id: format!(
                "zone_{}_{}",
                sanitize_layout_identifier(device_id),
                sanitize_layout_identifier(zone_name)
            ),
            name: format!("{device_name} · {zone_name}"),
            device_id: device_id.to_owned(),
            zone_name: Some(zone_name.to_string()),
            group_id: Some(group_id.clone()),
            position,
            size,
            rotation: 0.0,
            scale: 1.0,
            display_order: display_order_start
                + i32::try_from(offset).unwrap_or(i32::MAX.saturating_sub(display_order_start)),
            orientation,
            topology,
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: Some(SamplingMode::Bilinear),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape,
            shape_preset: Some("ableton-push2".to_owned()),
            attachment: None,
        });
    }

    if seeded_zones.is_empty() {
        return None;
    }

    Some(SeededDeviceLayout {
        group_id,
        group_name: device_name.to_owned(),
        group_color: PUSH2_GROUP_COLOR.to_owned(),
        zones: seeded_zones,
    })
}

pub(crate) fn attachment_zone_size(
    suggested: &AttachmentSuggestedZone,
    max_size: NormalizedPosition,
) -> NormalizedPosition {
    let units = attachment_visual_units(suggested);
    fit_visual_units(units, ATTACHMENT_MIN_SIZE, max_size)
}

pub(crate) fn attachment_zone_shape(category: &AttachmentCategory) -> Option<ZoneShape> {
    match category {
        AttachmentCategory::Fan
        | AttachmentCategory::Aio
        | AttachmentCategory::Heatsink
        | AttachmentCategory::Ring => Some(ZoneShape::Ring),
        AttachmentCategory::Strip
        | AttachmentCategory::Strimer
        | AttachmentCategory::Case
        | AttachmentCategory::Radiator
        | AttachmentCategory::Matrix => Some(ZoneShape::Rectangle),
        AttachmentCategory::Bulb | AttachmentCategory::Other(_) => None,
    }
}

pub(crate) fn normalize_layout_for_editor(mut layout: SpatialLayout) -> SpatialLayout {
    for zone in &mut layout.zones {
        zone.size = normalize_zone_size_for_editor(zone.position, zone.size, &zone.topology);
    }
    layout
}

pub(crate) fn repair_legacy_lcd_defaults(layout: &mut SpatialLayout) -> bool {
    let canvas_aspect = canvas_aspect_ratio(layout.canvas_width, layout.canvas_height);
    let mut changed = false;

    for zone in &mut layout.zones {
        if zone.shape_preset.as_deref() != Some("lcd-display") {
            continue;
        }

        let LedTopology::Matrix { width, height, .. } = zone.topology else {
            continue;
        };

        let units = VisualUnits::new(width.max(1) as f32, height.max(1) as f32);
        let legacy_size = fit_visual_units(units, DEVICE_MIN_SIZE, DEVICE_MAX_SIZE);
        if !approximately_equal_size(zone.size, legacy_size) {
            continue;
        }

        let corrected_size =
            fit_visual_units_for_canvas(units, DEVICE_MIN_SIZE, DEVICE_MAX_SIZE, canvas_aspect);
        if approximately_equal_size(zone.size, corrected_size) {
            continue;
        }

        zone.size = corrected_size;
        changed = true;
    }

    changed
}

pub(crate) fn normalize_zone_size_for_editor(
    position: NormalizedPosition,
    size: NormalizedPosition,
    topology: &LedTopology,
) -> NormalizedPosition {
    match topology {
        LedTopology::Strip { direction, .. } => clamp_strip_size(position, size, *direction),
        _ => size,
    }
}

pub(crate) fn drag_zone_to_position(
    layout: &mut SpatialLayout,
    zone_id: &str,
    desired_position: NormalizedPosition,
) -> bool {
    let Some(zone_index) = layout.zones.iter().position(|zone| zone.id == zone_id) else {
        return false;
    };

    let current_position = layout.zones[zone_index].position;
    let desired_position = NormalizedPosition::new(
        desired_position.x.clamp(0.0, 1.0),
        desired_position.y.clamp(0.0, 1.0),
    );

    let Some(group_id) = layout.zones[zone_index].group_id.clone() else {
        let clamped = clamp_zone_center(desired_position, layout.zones[zone_index].size);
        if current_position == clamped {
            return false;
        }
        layout.zones[zone_index].position = clamped;
        return true;
    };

    let member_indices = layout
        .zones
        .iter()
        .enumerate()
        .filter_map(|(index, zone)| (zone.group_id.as_deref() == Some(group_id.as_str())).then_some(index))
        .collect::<Vec<_>>();

    if member_indices.len() <= 1 {
        let clamped = clamp_zone_center(desired_position, layout.zones[zone_index].size);
        if current_position == clamped {
            return false;
        }
        layout.zones[zone_index].position = clamped;
        return true;
    }

    let desired_dx = desired_position.x - current_position.x;
    let desired_dy = desired_position.y - current_position.y;
    let mut min_dx = f32::NEG_INFINITY;
    let mut max_dx = f32::INFINITY;
    let mut min_dy = f32::NEG_INFINITY;
    let mut max_dy = f32::INFINITY;

    for index in &member_indices {
        let zone = &layout.zones[*index];
        min_dx = min_dx.max(zone.size.x * 0.5 - zone.position.x);
        max_dx = max_dx.min(1.0 - zone.size.x * 0.5 - zone.position.x);
        min_dy = min_dy.max(zone.size.y * 0.5 - zone.position.y);
        max_dy = max_dy.min(1.0 - zone.size.y * 0.5 - zone.position.y);
    }

    let clamped_dx = desired_dx.clamp(min_dx, max_dx);
    let clamped_dy = desired_dy.clamp(min_dy, max_dy);
    if clamped_dx.abs() <= GRID_EPSILON && clamped_dy.abs() <= GRID_EPSILON {
        return false;
    }

    for index in member_indices {
        let zone = &mut layout.zones[index];
        zone.position = NormalizedPosition::new(
            zone.position.x + clamped_dx,
            zone.position.y + clamped_dy,
        );
    }

    true
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

fn signal_visual_defaults(
    device_name: &str,
    led_count: u32,
    canvas_aspect: f32,
) -> Option<ZoneVisualDefaults> {
    let normalized_name = device_name.to_ascii_lowercase();

    if normalized_name.contains("basilisk v3 pro 35k")
        || normalized_name.contains("basilisk v3 pro")
    {
        return sparse_signal_defaults(
            BASILISK_V3_PRO_POINTS,
            BASILISK_V3_PRO_GRID,
            led_count,
            "razer-basilisk-v3-pro",
            canvas_aspect,
        );
    }

    if normalized_name.contains("basilisk v3 35k") || normalized_name.contains("basilisk v3") {
        return sparse_signal_defaults(
            BASILISK_V3_POINTS,
            BASILISK_V3_GRID,
            led_count,
            "razer-basilisk-v3",
            canvas_aspect,
        );
    }

    None
}

fn looks_like_push2(device_name: &str, zones: &[ZoneSummary]) -> bool {
    let normalized_name = device_name.to_ascii_lowercase();
    if !normalized_name.contains("push 2") {
        return false;
    }

    let zone_names = zones
        .iter()
        .map(|zone| zone.name.as_str())
        .collect::<std::collections::HashSet<_>>();
    zone_names.contains("Pads")
        && zone_names.contains("Buttons Above")
        && zone_names.contains("Buttons Below")
}

fn push2_zone_order() -> &'static [&'static str] {
    &[
        "White Buttons",
        "Transport",
        "Buttons Above",
        "Display",
        "Buttons Below",
        "Pads",
        "Scene Launch",
        "Touch Strip",
    ]
}

fn push2_zone_topology(zone: &ZoneSummary) -> LedTopology {
    match zone.name.as_str() {
        "Pads" => LedTopology::Matrix {
            width: 8,
            height: 8,
            serpentine: false,
            start_corner: Corner::BottomLeft,
        },
        "Buttons Above" => LedTopology::Strip {
            count: 8,
            direction: StripDirection::LeftToRight,
        },
        "Buttons Below" => LedTopology::Strip {
            count: 8,
            direction: StripDirection::LeftToRight,
        },
        "Scene Launch" => LedTopology::Strip {
            count: 8,
            direction: StripDirection::TopToBottom,
        },
        "Touch Strip" => LedTopology::Strip {
            count: 31,
            direction: StripDirection::BottomToTop,
        },
        "Transport" => LedTopology::Custom {
            positions: push2_transport_positions(),
        },
        "White Buttons" => LedTopology::Custom {
            positions: push2_white_button_positions(),
        },
        "Display" => LedTopology::Matrix {
            width: 960,
            height: 160,
            serpentine: false,
            start_corner: Corner::TopLeft,
        },
        _ => match zone.topology_hint.as_ref() {
            Some(ZoneTopologySummary::Strip) => LedTopology::Strip {
                count: u32::try_from(zone.led_count.max(1)).unwrap_or(u32::MAX),
                direction: StripDirection::LeftToRight,
            },
            Some(ZoneTopologySummary::Matrix { rows, cols }) => LedTopology::Matrix {
                width: *cols,
                height: *rows,
                serpentine: false,
                start_corner: Corner::TopLeft,
            },
            Some(ZoneTopologySummary::Ring { count }) => LedTopology::Ring {
                count: *count,
                start_angle: -FRAC_PI_2,
                direction: Winding::Clockwise,
            },
            Some(ZoneTopologySummary::Point) => LedTopology::Point,
            Some(ZoneTopologySummary::Display { width, height, .. }) => LedTopology::Matrix {
                width: *width,
                height: *height,
                serpentine: false,
                start_corner: Corner::TopLeft,
            },
            Some(ZoneTopologySummary::Custom) | None => LedTopology::Custom {
                positions: grid_points(&[(0, 0)], VisualUnits::new(1.0, 1.0)),
            },
        },
    }
}

fn push2_zone_geometry(
    zone_name: &str,
    footprint_size: NormalizedPosition,
) -> (NormalizedPosition, NormalizedPosition) {
    let rect = match zone_name {
        "White Buttons" => PUSH2_WHITE_BUTTONS_RECT,
        "Transport" => PUSH2_TRANSPORT_RECT,
        "Buttons Above" => FootprintRect::new(252.0, 106.0, 836.0, 41.0),
        "Display" => FootprintRect::new(246.0, 186.0, 852.0, 149.0),
        "Buttons Below" => FootprintRect::new(252.0, 390.0, 840.0, 36.0),
        "Pads" => FootprintRect::new(252.0, 458.0, 843.0, 595.0),
        "Scene Launch" => FootprintRect::new(1140.0, 458.0, 68.0, 595.0),
        "Touch Strip" => FootprintRect::new(96.0, 458.0, 78.0, 595.0),
        _ => FootprintRect::new(0.0, 0.0, 320.0, 80.0),
    };

    rect.to_canvas(footprint_size)
}

fn push2_transport_positions() -> Vec<NormalizedPosition> {
    normalize_points_in_rect(
        PUSH2_TRANSPORT_RECT,
        &[
            (39.0, 1030.0),
            (39.0, 965.0),
            (47.0, 128.0),
            (108.0, 128.0),
        ],
    )
}

fn push2_white_button_positions() -> Vec<NormalizedPosition> {
    normalize_points_in_rect(
        PUSH2_WHITE_BUTTONS_RECT,
        &[
            (1160.0, 389.0),
            (122.0, 389.0),
            (1260.0, 128.0),
            (1333.0, 794.0),
            (38.0, 463.0),
            (1249.0, 451.0),
            (1350.0, 451.0),
            (1299.0, 392.0),
            (1299.0, 507.0),
            (1299.0, 449.0),
            (1260.0, 1030.0),
            (1260.0, 852.0),
            (1333.0, 852.0),
            (1160.0, 195.0),
            (1160.0, 254.0),
            (1299.0, 1001.0),
            (1299.0, 923.0),
            (1333.0, 128.0),
            (82.0, 389.0),
            (1249.0, 962.0),
            (1350.0, 962.0),
            (38.0, 755.0),
            (38.0, 683.0),
            (38.0, 890.0),
            (38.0, 822.0),
            (1260.0, 194.0),
            (1260.0, 254.0),
            (1333.0, 194.0),
            (1333.0, 254.0),
            (38.0, 603.0),
            (38.0, 529.0),
            (38.0, 196.0),
            (38.0, 259.0),
            (1260.0, 726.0),
            (1333.0, 726.0),
            (1260.0, 793.0),
            (39.0, 389.0),
        ],
    )
}

fn sparse_signal_defaults(
    points: &[(u32, u32)],
    grid: VisualUnits,
    led_count: u32,
    shape_preset: &str,
    canvas_aspect: f32,
) -> Option<ZoneVisualDefaults> {
    let positions = grid_points(points, grid);
    #[allow(clippy::cast_possible_truncation)]
    if positions.len() as u32 != led_count {
        return None;
    }

    Some(ZoneVisualDefaults {
        topology: LedTopology::Custom { positions },
        size: fit_visual_units_for_canvas(grid, DEVICE_MIN_SIZE, DEVICE_MAX_SIZE, canvas_aspect),
        orientation: Some(Orientation::Horizontal),
        shape: Some(ZoneShape::Rectangle),
        shape_preset: Some(shape_preset.to_owned()),
    })
}

fn matrix_defaults(
    rows: u32,
    cols: u32,
    shape_preset: Option<&str>,
    canvas_aspect: f32,
) -> ZoneVisualDefaults {
    let grid = VisualUnits::new(cols.max(1) as f32, rows.max(1) as f32);
    let aspect = grid.aspect_ratio();

    ZoneVisualDefaults {
        topology: LedTopology::Matrix {
            width: cols.max(1),
            height: rows.max(1),
            serpentine: false,
            start_corner: Corner::TopLeft,
        },
        size: fit_visual_units_for_canvas(grid, DEVICE_MIN_SIZE, DEVICE_MAX_SIZE, canvas_aspect),
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
    canvas_aspect: f32,
) -> ZoneVisualDefaults {
    let topology = LedTopology::Strip {
        count: count.max(1),
        direction,
    };

    ZoneVisualDefaults {
        topology,
        size: fit_visual_units_for_canvas(
            topology_visual_units(&LedTopology::Strip {
                count: count.max(1),
                direction,
            }),
            NormalizedPosition::new(0.10, 0.02),
            NormalizedPosition::new(0.34, 0.12),
            canvas_aspect,
        ),
        orientation: Some(match direction {
            StripDirection::LeftToRight | StripDirection::RightToLeft => Orientation::Horizontal,
            StripDirection::TopToBottom | StripDirection::BottomToTop => Orientation::Vertical,
        }),
        shape: Some(ZoneShape::Rectangle),
        shape_preset: shape_preset.map(str::to_owned),
    }
}

fn ring_defaults(count: u32, shape_preset: Option<&str>, canvas_aspect: f32) -> ZoneVisualDefaults {
    ZoneVisualDefaults {
        topology: LedTopology::Ring {
            count: count.max(1),
            start_angle: -FRAC_PI_2,
            direction: Winding::Clockwise,
        },
        size: fit_visual_units_for_canvas(
            VisualUnits::new(1.0, 1.0),
            DEVICE_RING_SIZE,
            DEVICE_RING_SIZE,
            canvas_aspect,
        ),
        orientation: Some(Orientation::Radial),
        shape: Some(ZoneShape::Ring),
        shape_preset: shape_preset.map(str::to_owned),
    }
}

fn point_defaults(canvas_aspect: f32) -> ZoneVisualDefaults {
    ZoneVisualDefaults {
        topology: LedTopology::Point,
        size: fit_visual_units_for_canvas(
            VisualUnits::new(1.0, 1.0),
            NormalizedPosition::new(0.08, 0.08),
            NormalizedPosition::new(0.08, 0.08),
            canvas_aspect,
        ),
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
    fit_aspect_ratio(units.aspect_ratio(), min_size, max_size)
}

fn fit_visual_units_for_canvas(
    units: VisualUnits,
    min_size: NormalizedPosition,
    max_size: NormalizedPosition,
    canvas_aspect: f32,
) -> NormalizedPosition {
    fit_aspect_ratio(
        (units.aspect_ratio() / canvas_aspect.max(GRID_EPSILON)).max(GRID_EPSILON),
        min_size,
        max_size,
    )
}

fn fit_aspect_ratio(
    aspect: f32,
    min_size: NormalizedPosition,
    max_size: NormalizedPosition,
) -> NormalizedPosition {
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

fn canvas_aspect_ratio(canvas_width: u32, canvas_height: u32) -> f32 {
    let width = f32::from(u16::try_from(canvas_width.max(1)).unwrap_or(u16::MAX));
    let height = f32::from(u16::try_from(canvas_height.max(1)).unwrap_or(u16::MAX));
    (width / height).max(GRID_EPSILON)
}

fn approximately_equal_size(left: NormalizedPosition, right: NormalizedPosition) -> bool {
    (left.x - right.x).abs() <= 0.001 && (left.y - right.y).abs() <= 0.001
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

fn attachment_visual_units(suggested: &AttachmentSuggestedZone) -> VisualUnits {
    match suggested.category {
        AttachmentCategory::Fan
        | AttachmentCategory::Aio
        | AttachmentCategory::Heatsink
        | AttachmentCategory::Ring
        | AttachmentCategory::Bulb => VisualUnits::new(1.0, 1.0),
        AttachmentCategory::Strimer | AttachmentCategory::Matrix => {
            topology_visual_units(&suggested.topology)
        }
        AttachmentCategory::Strip
        | AttachmentCategory::Case
        | AttachmentCategory::Radiator
        | AttachmentCategory::Other(_) => topology_visual_units(&suggested.topology),
    }
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

fn normalize_points_in_rect(
    rect: FootprintRect,
    points: &[(f32, f32)],
) -> Vec<NormalizedPosition> {
    points
        .iter()
        .map(|&(x, y)| {
            NormalizedPosition::new(
                ((x - rect.x) / rect.width).clamp(0.0, 1.0),
                ((y - rect.y) / rect.height).clamp(0.0, 1.0),
            )
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FootprintRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl FootprintRect {
    const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn to_canvas(self, footprint_size: NormalizedPosition) -> (NormalizedPosition, NormalizedPosition) {
        let size = NormalizedPosition::new(
            footprint_size.x * (self.width / PUSH2_FOOTPRINT_GRID.width),
            footprint_size.y * (self.height / PUSH2_FOOTPRINT_GRID.height),
        );
        let left = PUSH2_FOOTPRINT_CENTER.x - footprint_size.x * 0.5
            + footprint_size.x * (self.x / PUSH2_FOOTPRINT_GRID.width);
        let top = PUSH2_FOOTPRINT_CENTER.y - footprint_size.y * 0.5
            + footprint_size.y * (self.y / PUSH2_FOOTPRINT_GRID.height);
        let position = NormalizedPosition::new(left + size.x * 0.5, top + size.y * 0.5);
        (position, size)
    }
}

fn sanitize_layout_identifier(raw: &str) -> String {
    raw.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
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
        position
            .x
            .clamp(size.x.mul_add(0.5, 0.0).min(1.0), (1.0 - size.x * 0.5).max(0.0)),
        position
            .y
            .clamp(size.y.mul_add(0.5, 0.0).min(1.0), (1.0 - size.y * 0.5).max(0.0)),
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
