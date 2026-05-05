//! Shared layout geometry helpers for default footprints and proportional resizing.
#![allow(
    dead_code,
    reason = "Some helpers are pre-built for upcoming editor features."
)]

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use hypercolor_types::attachment::{AttachmentCategory, AttachmentSuggestedZone};
use hypercolor_types::spatial::{
    Corner, DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, Orientation, SamplingMode,
    SpatialLayout, StripDirection, Winding, ZoneAttachment, ZoneShape,
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
const PUSH2_TRANSPORT_RECT: FootprintRect = FootprintRect::new(16.0, 74.0, 130.0, 959.0);
const PUSH2_WHITE_BUTTONS_RECT: FootprintRect = FootprintRect::new(16.0, 128.0, 1334.0, 903.0);
const ATTACHMENT_SLOT_GAP_FRACTION: f32 = 0.06;

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
    pub zones: Vec<DeviceZone>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SeededAttachmentLayout {
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
            brightness: None,
        });
    }

    if seeded_zones.is_empty() {
        return None;
    }

    Some(SeededDeviceLayout {
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

#[allow(clippy::cast_precision_loss)]
pub(crate) fn seeded_attachment_layout(
    device_id: &str,
    _device_name: &str,
    suggested_zones: &[AttachmentSuggestedZone],
    display_order_start: i32,
) -> SeededAttachmentLayout {
    if suggested_zones.is_empty() {
        return SeededAttachmentLayout { zones: Vec::new() };
    }

    let mut slots = suggested_zones
        .iter()
        .cloned()
        .fold(
            std::collections::BTreeMap::<String, Vec<AttachmentSuggestedZone>>::new(),
            |mut acc, zone| {
                acc.entry(zone.slot_id.clone()).or_default().push(zone);
                acc
            },
        )
        .into_iter()
        .collect::<Vec<_>>();

    for (_, zones) in &mut slots {
        zones.sort_by(|left, right| {
            left.led_start
                .cmp(&right.led_start)
                .then_with(|| left.instance.cmp(&right.instance))
                .then_with(|| left.name.cmp(&right.name))
        });
    }

    let slot_count = slots.len();
    let columns = slot_count.clamp(1, 3);
    let rows = slot_count.div_ceil(columns);
    let cell_width = 0.78 / columns as f32;
    let cell_height = 0.68 / rows as f32;

    let mut zones = Vec::new();

    for (slot_index, (_slot_id, slot_zones)) in slots.into_iter().enumerate() {
        let row = slot_index / columns;
        let column = slot_index % columns;
        let cell_center = NormalizedPosition::new(
            0.12 + cell_width * (column as f32 + 0.5),
            0.18 + cell_height * (row as f32 + 0.5),
        );
        let max_size = NormalizedPosition::new(cell_width * 0.86, cell_height * 0.82);

        let placements = attachment_slot_placements(&slot_zones, cell_center, max_size);
        let slot_display_order_start =
            display_order_start + i32::try_from(zones.len()).unwrap_or(i32::MAX);
        for (slot_offset, (suggested, (position, size))) in
            slot_zones.into_iter().zip(placements).enumerate()
        {
            let shape = attachment_zone_shape(&suggested.category);
            zones.push(DeviceZone {
                id: attachment_zone_id(device_id, &suggested),
                name: suggested.name.clone(),
                device_id: device_id.to_owned(),
                zone_name: Some(suggested.slot_id.clone()),
                position,
                size,
                rotation: 0.0,
                scale: 1.0,
                orientation: if matches!(shape, Some(ZoneShape::Ring)) {
                    Some(Orientation::Radial)
                } else {
                    orientation_for_attachment_topology(&suggested.topology)
                },
                topology: suggested.topology.clone(),
                led_positions: Vec::new(),
                led_mapping: suggested.led_mapping.clone(),
                sampling_mode: None,
                edge_behavior: None,
                shape,
                shape_preset: None,
                display_order: slot_display_order_start
                    + i32::try_from(slot_offset).unwrap_or(i32::MAX),
                attachment: Some(ZoneAttachment {
                    template_id: suggested.template_id.clone(),
                    slot_id: suggested.slot_id.clone(),
                    instance: suggested.instance,
                    led_start: Some(suggested.led_start),
                    led_count: Some(suggested.led_count),
                    led_mapping: suggested.led_mapping.clone(),
                }),
                brightness: None,
            });
        }
    }

    SeededAttachmentLayout { zones }
}

pub(crate) fn normalize_layout_for_editor(mut layout: SpatialLayout) -> SpatialLayout {
    for zone in &mut layout.zones {
        zone.size = normalize_zone_size_for_editor(zone.position, zone.size, &zone.topology);
    }
    layout
}

pub(crate) fn normalize_zone_size_for_editor(
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

pub(crate) fn set_zone_rotation(layout: &mut SpatialLayout, zone_id: &str, rotation: f32) -> bool {
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

pub(crate) fn resize_zone_from_handle(
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
        &[(39.0, 1030.0), (39.0, 965.0), (47.0, 128.0), (108.0, 128.0)],
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
            (1333.0, 1030.0),
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

fn attachment_slot_placements(
    zones: &[AttachmentSuggestedZone],
    center: NormalizedPosition,
    max_size: NormalizedPosition,
) -> Vec<(NormalizedPosition, NormalizedPosition)> {
    if zones.len() <= 1 {
        return zones
            .iter()
            .map(|zone| {
                let size = normalize_zone_size_for_editor(
                    center,
                    attachment_zone_size(zone, max_size),
                    &zone.topology,
                );
                (center, size)
            })
            .collect();
    }

    let slot_gap = (max_size.x * ATTACHMENT_SLOT_GAP_FRACTION).clamp(0.012, 0.03);
    let aspects = zones
        .iter()
        .map(|zone| attachment_visual_units(zone).aspect_ratio())
        .collect::<Vec<_>>();
    let total_aspect = aspects.iter().sum::<f32>().max(GRID_EPSILON);
    let total_gap = slot_gap * (zones.len().saturating_sub(1) as f32);
    let usable_width = (max_size.x - total_gap).max(GRID_EPSILON);
    let row_height = (usable_width / total_aspect)
        .min(max_size.y)
        .max(GRID_EPSILON);

    let widths = aspects
        .iter()
        .map(|aspect| (row_height * *aspect).max(GRID_EPSILON))
        .collect::<Vec<_>>();
    let total_width = widths.iter().sum::<f32>() + total_gap;
    let mut cursor = center.x - total_width * 0.5;

    zones
        .iter()
        .zip(widths)
        .map(|(zone, width)| {
            let position = NormalizedPosition::new(cursor + width * 0.5, center.y);
            let size = normalize_zone_size_for_editor(
                position,
                NormalizedPosition::new(width, row_height),
                &zone.topology,
            );
            cursor += width + slot_gap;
            (position, size)
        })
        .collect()
}

fn orientation_for_attachment_topology(topology: &LedTopology) -> Option<Orientation> {
    match topology {
        LedTopology::Strip { .. } => Some(Orientation::Horizontal),
        LedTopology::Ring { .. } | LedTopology::ConcentricRings { .. } | LedTopology::Point => {
            Some(Orientation::Radial)
        }
        LedTopology::Matrix { .. }
        | LedTopology::PerimeterLoop { .. }
        | LedTopology::Custom { .. } => None,
    }
}

fn attachment_zone_id(device_id: &str, suggested: &AttachmentSuggestedZone) -> String {
    format!(
        "attachment-{}-{}-{}-{}",
        sanitize_layout_identifier(device_id),
        sanitize_layout_identifier(&suggested.slot_id),
        suggested.led_start,
        suggested.instance
    )
}

fn humanize_slot_id(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut previous_space = true;

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            if previous_space {
                out.push(ch.to_ascii_uppercase());
            } else {
                out.push(ch);
            }
            previous_space = false;
            continue;
        }

        if !previous_space {
            out.push(' ');
            previous_space = true;
        }
    }

    out.trim().to_owned()
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

fn normalize_points_in_rect(rect: FootprintRect, points: &[(f32, f32)]) -> Vec<NormalizedPosition> {
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

    fn to_canvas(
        self,
        footprint_size: NormalizedPosition,
    ) -> (NormalizedPosition, NormalizedPosition) {
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

// ── Compound geometry ────────────────────────────────────────────────────

/// Axis-aligned bounding box enclosing a set of zones.
#[derive(Debug, Clone)]
pub(crate) struct CompoundBounds {
    pub center: NormalizedPosition,
    pub size: NormalizedPosition,
}

/// Compute the axis-aligned bounding box of all zones in `zone_ids`.
pub(crate) fn compound_bounding_box(
    layout: &SpatialLayout,
    zone_ids: &std::collections::HashSet<String>,
) -> Option<CompoundBounds> {
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    let mut found = false;

    for zone in &layout.zones {
        if !zone_ids.contains(&zone.id) {
            continue;
        }
        found = true;
        let half_w = zone.size.x * 0.5;
        let half_h = zone.size.y * 0.5;
        min_x = min_x.min(zone.position.x - half_w);
        min_y = min_y.min(zone.position.y - half_h);
        max_x = max_x.max(zone.position.x + half_w);
        max_y = max_y.max(zone.position.y + half_h);
    }

    if !found {
        return None;
    }

    Some(CompoundBounds {
        center: NormalizedPosition::new((min_x + max_x) * 0.5, (min_y + max_y) * 0.5),
        size: NormalizedPosition::new(max_x - min_x, max_y - min_y),
    })
}

/// Translate all zones by a delta from their initial positions, clamping each to canvas bounds.
pub(crate) fn translate_zones(
    layout: &mut SpatialLayout,
    initial_positions: &[(String, NormalizedPosition)],
    delta: NormalizedPosition,
) -> bool {
    let mut changed = false;
    for (id, initial_pos) in initial_positions {
        if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == *id) {
            let desired = NormalizedPosition::new(
                (initial_pos.x + delta.x).clamp(0.0, 1.0),
                (initial_pos.y + delta.y).clamp(0.0, 1.0),
            );
            let clamped = clamp_zone_center(desired, zone.size);
            if zone.position != clamped {
                zone.position = clamped;
                changed = true;
            }
        }
    }
    changed
}

// ── Group transforms ─────────────────────────────────────────────────────

/// Centroid (average position) of all zones in `zone_ids`.
pub(crate) fn group_centroid(
    layout: &SpatialLayout,
    zone_ids: &std::collections::HashSet<String>,
) -> Option<NormalizedPosition> {
    let (sum_x, sum_y, count) = layout
        .zones
        .iter()
        .filter(|z| zone_ids.contains(&z.id))
        .fold((0.0f32, 0.0f32, 0u32), |(sx, sy, n), z| {
            (sx + z.position.x, sy + z.position.y, n + 1)
        });
    (count > 0).then(|| NormalizedPosition::new(sum_x / count as f32, sum_y / count as f32))
}

/// Translate a group so its centroid lands at `target`, preserving relative positions.
pub(crate) fn translate_group(
    layout: &mut SpatialLayout,
    zone_ids: &std::collections::HashSet<String>,
    target: NormalizedPosition,
) -> bool {
    let Some(centroid) = group_centroid(layout, zone_ids) else {
        return false;
    };
    let delta = NormalizedPosition::new(target.x - centroid.x, target.y - centroid.y);
    if delta.x.abs() < GRID_EPSILON && delta.y.abs() < GRID_EPSILON {
        return false;
    }
    let mut changed = false;
    for zone in &mut layout.zones {
        if !zone_ids.contains(&zone.id) {
            continue;
        }
        let desired = NormalizedPosition::new(
            (zone.position.x + delta.x).clamp(0.0, 1.0),
            (zone.position.y + delta.y).clamp(0.0, 1.0),
        );
        let clamped = clamp_zone_center(desired, zone.size);
        if zone.position != clamped {
            zone.position = clamped;
            changed = true;
        }
    }
    changed
}

/// Rotate all zones in `zone_ids` by `delta_radians` around their group centroid.
///
/// Each zone's position orbits the centroid and its individual rotation is offset
/// by the same delta — so fans on a ring stay oriented correctly.
pub(crate) fn rotate_group(
    layout: &mut SpatialLayout,
    zone_ids: &std::collections::HashSet<String>,
    delta_radians: f32,
) -> bool {
    if delta_radians.abs() < GRID_EPSILON {
        return false;
    }
    let Some(centroid) = group_centroid(layout, zone_ids) else {
        return false;
    };
    let (sin_d, cos_d) = delta_radians.sin_cos();
    let mut changed = false;
    for zone in &mut layout.zones {
        if !zone_ids.contains(&zone.id) {
            continue;
        }
        // Orbit position around centroid
        let dx = zone.position.x - centroid.x;
        let dy = zone.position.y - centroid.y;
        let rx = dx.mul_add(cos_d, -(dy * sin_d));
        let ry = dx.mul_add(sin_d, dy * cos_d);
        let desired = NormalizedPosition::new(
            (centroid.x + rx).clamp(0.0, 1.0),
            (centroid.y + ry).clamp(0.0, 1.0),
        );
        let clamped = clamp_zone_center(desired, zone.size);
        if zone.position != clamped {
            zone.position = clamped;
            changed = true;
        }
        // Rotate the zone itself
        let new_rotation = normalize_rotation(zone.rotation + delta_radians);
        if (new_rotation - zone.rotation).abs() > GRID_EPSILON {
            zone.rotation = new_rotation;
            changed = true;
        }
    }
    changed
}

/// Scale all zones in `zone_ids` by `scale_ratio` around their group centroid.
///
/// Each zone's distance from the centroid is multiplied by `scale_ratio`, and
/// the zone's individual `scale` field is multiplied by the same factor.
pub(crate) fn scale_group(
    layout: &mut SpatialLayout,
    zone_ids: &std::collections::HashSet<String>,
    scale_ratio: f32,
) -> bool {
    if (scale_ratio - 1.0).abs() < GRID_EPSILON || scale_ratio <= 0.0 {
        return false;
    }
    let Some(centroid) = group_centroid(layout, zone_ids) else {
        return false;
    };
    let mut changed = false;
    for zone in &mut layout.zones {
        if !zone_ids.contains(&zone.id) {
            continue;
        }
        // Scale distance from centroid
        let dx = zone.position.x - centroid.x;
        let dy = zone.position.y - centroid.y;
        let desired = NormalizedPosition::new(
            (centroid.x + dx * scale_ratio).clamp(0.0, 1.0),
            (centroid.y + dy * scale_ratio).clamp(0.0, 1.0),
        );
        let clamped = clamp_zone_center(desired, zone.size);
        if zone.position != clamped {
            zone.position = clamped;
            changed = true;
        }
        // Scale the zone itself
        let new_scale = (zone.scale * scale_ratio).clamp(0.1, 5.0);
        if (new_scale - zone.scale).abs() > GRID_EPSILON {
            zone.scale = new_scale;
            changed = true;
        }
    }
    changed
}

// ── Group alignment / distribute / pack / mirror ─────────────────────────

/// Which axis a group operation acts on. `X` is horizontal (left/right),
/// `Y` is vertical (top/bottom).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AlignAxis {
    X,
    Y,
}

/// Which edge or center to align selected zones against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AlignAnchor {
    /// Left edge (X axis) or top edge (Y axis).
    Min,
    /// Bbox center on the chosen axis.
    Center,
    /// Right edge (X axis) or bottom edge (Y axis).
    Max,
}

/// Align each zone in `zone_ids` to a common edge or center of the group's
/// bounding box on `axis`. Zones on the other axis are left untouched.
pub(crate) fn align_group(
    layout: &mut SpatialLayout,
    zone_ids: &std::collections::HashSet<String>,
    axis: AlignAxis,
    anchor: AlignAnchor,
) -> bool {
    let Some(bounds) = compound_bounding_box(layout, zone_ids) else {
        return false;
    };
    let (bbox_min, bbox_max) = match axis {
        AlignAxis::X => (
            bounds.center.x - bounds.size.x * 0.5,
            bounds.center.x + bounds.size.x * 0.5,
        ),
        AlignAxis::Y => (
            bounds.center.y - bounds.size.y * 0.5,
            bounds.center.y + bounds.size.y * 0.5,
        ),
    };
    let bbox_center = (bbox_min + bbox_max) * 0.5;

    let mut changed = false;
    for zone in &mut layout.zones {
        if !zone_ids.contains(&zone.id) {
            continue;
        }
        let half = match axis {
            AlignAxis::X => zone.size.x * 0.5,
            AlignAxis::Y => zone.size.y * 0.5,
        };
        let target = match anchor {
            AlignAnchor::Min => bbox_min + half,
            AlignAnchor::Center => bbox_center,
            AlignAnchor::Max => bbox_max - half,
        };
        let desired = match axis {
            AlignAxis::X => NormalizedPosition::new(target.clamp(0.0, 1.0), zone.position.y),
            AlignAxis::Y => NormalizedPosition::new(zone.position.x, target.clamp(0.0, 1.0)),
        };
        let clamped = clamp_zone_center(desired, zone.size);
        if zone.position != clamped {
            zone.position = clamped;
            changed = true;
        }
    }
    changed
}

/// Distribute zones evenly along `axis` so the space BETWEEN their edges
/// is equal. The first and last zones (by position on `axis`) keep their
/// positions; the middle zones are repositioned.
///
/// No-op for fewer than three zones.
pub(crate) fn distribute_group(
    layout: &mut SpatialLayout,
    zone_ids: &std::collections::HashSet<String>,
    axis: AlignAxis,
) -> bool {
    let mut sorted: Vec<(String, f32, f32)> = layout
        .zones
        .iter()
        .filter(|z| zone_ids.contains(&z.id))
        .map(|z| {
            let (pos, size) = match axis {
                AlignAxis::X => (z.position.x, z.size.x),
                AlignAxis::Y => (z.position.y, z.size.y),
            };
            (z.id.clone(), pos, size)
        })
        .collect();

    if sorted.len() < 3 {
        return false;
    }
    sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let first_left = sorted[0].1 - sorted[0].2 * 0.5;
    let last = sorted.last().expect("len >= 3");
    let last_right = last.1 + last.2 * 0.5;
    let total_span = last_right - first_left;
    let total_size: f32 = sorted.iter().map(|(_, _, s)| *s).sum();
    let gap = (total_span - total_size) / (sorted.len() as f32 - 1.0);

    let mut cursor_left = first_left;
    let mut targets: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
    for (id, _, size) in &sorted {
        targets.insert(id.clone(), cursor_left + size * 0.5);
        cursor_left += size + gap;
    }

    let mut changed = false;
    for zone in &mut layout.zones {
        let Some(&target) = targets.get(&zone.id) else {
            continue;
        };
        let desired = match axis {
            AlignAxis::X => NormalizedPosition::new(target.clamp(0.0, 1.0), zone.position.y),
            AlignAxis::Y => NormalizedPosition::new(zone.position.x, target.clamp(0.0, 1.0)),
        };
        let clamped = clamp_zone_center(desired, zone.size);
        if zone.position != clamped {
            zone.position = clamped;
            changed = true;
        }
    }
    changed
}

/// Pack zones edge-to-edge along `axis`, removing all gaps. Zones are
/// ordered by their current position on `axis` and the first zone anchors
/// the sequence.
pub(crate) fn pack_group(
    layout: &mut SpatialLayout,
    zone_ids: &std::collections::HashSet<String>,
    axis: AlignAxis,
) -> bool {
    let mut sorted: Vec<(String, f32, f32)> = layout
        .zones
        .iter()
        .filter(|z| zone_ids.contains(&z.id))
        .map(|z| {
            let (pos, size) = match axis {
                AlignAxis::X => (z.position.x, z.size.x),
                AlignAxis::Y => (z.position.y, z.size.y),
            };
            (z.id.clone(), pos, size)
        })
        .collect();
    if sorted.len() < 2 {
        return false;
    }
    sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut cursor_left = sorted[0].1 - sorted[0].2 * 0.5;
    let mut targets: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
    for (id, _, size) in &sorted {
        targets.insert(id.clone(), cursor_left + size * 0.5);
        cursor_left += size;
    }

    let mut changed = false;
    for zone in &mut layout.zones {
        let Some(&target) = targets.get(&zone.id) else {
            continue;
        };
        let desired = match axis {
            AlignAxis::X => NormalizedPosition::new(target.clamp(0.0, 1.0), zone.position.y),
            AlignAxis::Y => NormalizedPosition::new(zone.position.x, target.clamp(0.0, 1.0)),
        };
        let clamped = clamp_zone_center(desired, zone.size);
        if zone.position != clamped {
            zone.position = clamped;
            changed = true;
        }
    }
    changed
}

/// Mirror zones across the group centroid on `axis`. Positions flip
/// around the centroid and each zone's rotation is reflected through the
/// same axis so a mirrored strip keeps its apparent orientation.
///
/// - `AlignAxis::X` reflects across a **vertical** line through the
///   centroid, mapping rotation θ to π − θ.
/// - `AlignAxis::Y` reflects across a **horizontal** line through the
///   centroid, mapping rotation θ to −θ.
///
/// No-op for fewer than two zones.
pub(crate) fn mirror_group(
    layout: &mut SpatialLayout,
    zone_ids: &std::collections::HashSet<String>,
    axis: AlignAxis,
) -> bool {
    if zone_ids.len() < 2 {
        return false;
    }
    let Some(centroid) = group_centroid(layout, zone_ids) else {
        return false;
    };
    let mut changed = false;
    for zone in &mut layout.zones {
        if !zone_ids.contains(&zone.id) {
            continue;
        }
        let desired = match axis {
            AlignAxis::X => NormalizedPosition::new(
                (2.0 * centroid.x - zone.position.x).clamp(0.0, 1.0),
                zone.position.y,
            ),
            AlignAxis::Y => NormalizedPosition::new(
                zone.position.x,
                (2.0 * centroid.y - zone.position.y).clamp(0.0, 1.0),
            ),
        };
        let clamped = clamp_zone_center(desired, zone.size);
        if zone.position != clamped {
            zone.position = clamped;
            changed = true;
        }
        let mirrored_rot = match axis {
            AlignAxis::X => normalize_rotation(PI - zone.rotation),
            AlignAxis::Y => normalize_rotation(-zone.rotation),
        };
        if (mirrored_rot - zone.rotation).abs() > GRID_EPSILON {
            zone.rotation = mirrored_rot;
            changed = true;
        }
    }
    changed
}
