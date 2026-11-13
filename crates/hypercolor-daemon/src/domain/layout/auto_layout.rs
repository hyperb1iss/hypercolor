use std::collections::HashSet;

use hypercolor_core::spatial::generate_positions;
use hypercolor_types::attachment::channel_name_matches_slot_alias;
use hypercolor_types::device::{DeviceInfo, DeviceTopologyHint, SegmentPlacement};
use hypercolor_types::scene::{Zone, ZoneRole};
use hypercolor_types::spatial::{
    Corner, EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection, Winding, ZoneShape,
};
#[must_use]
/// Adopt into `layout` every output a persisted primary zone holds that the
/// layout does not: a member assigned in Studio is minted into the scene's
/// zone, and the layout store only learns of it here. Returns how many were
/// adopted.
pub(super) fn adopt_primary_zone_outputs(layout: &mut SpatialLayout, zones: &[Zone]) -> usize {
    let mut adopted = 0;
    for zone in zones
        .iter()
        .filter(|zone| zone.role == ZoneRole::Primary && zone.display_target.is_none())
    {
        for output in &zone.layout.zones {
            if layout.zones.iter().any(|existing| existing.id == output.id) {
                continue;
            }
            layout.zones.push(output.clone());
            adopted += 1;
        }
    }
    adopted
}

pub(super) fn append_auto_layout_zones_for_device(
    layout: &mut SpatialLayout,
    layout_device_id: &str,
    device_info: &DeviceInfo,
) -> usize {
    let eligible_segments = device_info
        .segments
        .iter()
        .filter(|segment| {
            segment.led_count > 0 && !matches!(segment.topology, DeviceTopologyHint::Display { .. })
        })
        .cloned()
        .collect::<Vec<_>>();
    if eligible_segments.is_empty() {
        return 0;
    }

    let existing_device_count = layout
        .zones
        .iter()
        .map(|zone| zone.device_id.as_str())
        .collect::<HashSet<_>>()
        .len();
    let slot_center = auto_layout_slot_center(existing_device_count);
    let physical = SegmentPlacement::fit_all(
        &eligible_segments
            .iter()
            .map(|segment| segment.layout_hint.as_ref().and_then(|hint| hint.placement))
            .collect::<Vec<_>>(),
        slot_center,
        NormalizedPosition::new(0.26, 0.18),
        (layout.canvas_width, layout.canvas_height),
    );

    for (index, segment_info) in eligible_segments.iter().enumerate() {
        let layout_hint = segment_info.layout_hint.as_ref();
        let topology = layout_hint
            .and_then(|hint| hint.topology.clone())
            .unwrap_or_else(|| spatial_topology_for_segment(segment_info));
        let (default_position, default_size) = auto_layout_geometry(
            slot_center,
            index,
            eligible_segments.len(),
            &segment_info.topology,
        );
        let default_position = layout_hint
            .filter(|hint| hint.co_located)
            .map_or(default_position, |_| slot_center);
        let default_size = layout_hint
            .and_then(|hint| hint.size)
            .unwrap_or(default_size);
        let placement = physical[index];
        let position = placement.map_or(default_position, |placement| placement.position);
        let size = placement.map_or(default_size, |placement| placement.size);
        let zone_id = unique_auto_zone_id(layout, layout_device_id, &segment_info.name);
        let zone_name = if eligible_segments.len() == 1 {
            device_info.name.clone()
        } else {
            format!("{}: {}", device_info.name, segment_info.name)
        };

        layout.zones.push(Output {
            id: zone_id,
            name: zone_name,
            device_id: layout_device_id.to_owned(),
            zone_name: Some(segment_info.name.clone()),
            position,
            size,
            rotation: placement.map_or(0.0, |placement| placement.rotation),
            scale: 1.0,
            display_order: 0,
            orientation: None,
            topology: topology.clone(),
            led_positions: generate_positions(&topology),
            led_mapping: None,
            sampling_mode: Some(SamplingMode::Bilinear),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape: layout_hint
                .and_then(|hint| hint.shape.clone())
                .or_else(|| auto_layout_shape(&segment_info.topology)),
            shape_preset: None,
            attachment: None,
            brightness: None,
        });
    }

    eligible_segments.len()
}

/// How much of the canvas a screen's box takes on its longer side when
/// it is first given one.
const SCREEN_OUTPUT_EXTENT: f32 = 0.2;

/// Append one output per screen of a device: a box of the panel's own
/// shape, centered on the device's outputs `layout` already holds, or on
/// the canvas when it holds none. Returns how many were appended.
///
/// Automatic layout never calls this. A screen's box is where its backdrop
/// is cropped from, so it joins a canvas only when someone assigns it.
pub(super) fn append_screen_outputs_for_device(
    layout: &mut SpatialLayout,
    layout_device_id: &str,
    device_info: &DeviceInfo,
) -> usize {
    let held: Vec<NormalizedPosition> = layout
        .zones
        .iter()
        .filter(|output| output.device_id == layout_device_id)
        .map(|output| output.position)
        .collect();
    let center = if held.is_empty() {
        NormalizedPosition::new(0.5, 0.5)
    } else {
        let count = f32::from(u16::try_from(held.len()).unwrap_or(u16::MAX));
        NormalizedPosition::new(
            held.iter().map(|position| position.x).sum::<f32>() / count,
            held.iter().map(|position| position.y).sum::<f32>() / count,
        )
    };
    let lights = device_info
        .segments
        .iter()
        .filter(|segment| !matches!(segment.topology, DeviceTopologyHint::Display { .. }))
        .count();
    let canvas = (
        f64::from(layout.canvas_width.max(1)),
        f64::from(layout.canvas_height.max(1)),
    );
    let mut appended = 0;
    for segment in &device_info.segments {
        let DeviceTopologyHint::Display { width, height, .. } = segment.topology else {
            continue;
        };
        // The panel's shape in canvas pixels, fitted to the box a screen
        // arrives in, then back to the canvas's own units.
        let panel = (f64::from(width.max(1)), f64::from(height.max(1)));
        let extent = f64::from(SCREEN_OUTPUT_EXTENT);
        let fit = (extent * canvas.0 / panel.0).min(extent * canvas.1 / panel.1);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a fraction of the canvas, at most the arrival extent"
        )]
        let size = NormalizedPosition::new(
            (panel.0 * fit / canvas.0) as f32,
            (panel.1 * fit / canvas.1) as f32,
        );
        let name = if lights == 0 {
            device_info.name.clone()
        } else {
            format!("{}: {}", device_info.name, segment.name)
        };
        layout.zones.push(Output {
            id: unique_auto_zone_id(layout, layout_device_id, &segment.name),
            name,
            device_id: layout_device_id.to_owned(),
            zone_name: Some(segment.name.clone()),
            position: center,
            size,
            rotation: 0.0,
            scale: 1.0,
            display_order: 0,
            orientation: None,
            // A screen has no LEDs to sample: one cell stands for the panel.
            topology: LedTopology::Matrix {
                width: 1,
                height: 1,
                serpentine: false,
                start_corner: Corner::TopLeft,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: Some(SamplingMode::Bilinear),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape: auto_layout_shape(&segment.topology),
            shape_preset: None,
            attachment: None,
            brightness: None,
        });
        appended += 1;
    }
    appended
}

#[must_use]
pub(super) fn reconcile_auto_layout_zones_for_device(
    layout: &mut SpatialLayout,
    layout_device_id: &str,
    device_info: &DeviceInfo,
) -> usize {
    let auto_zone_prefix = format!("auto-{}-", sanitize_auto_layout_component(layout_device_id));
    let eligible_segments = device_info
        .segments
        .iter()
        .filter(|segment| {
            segment.led_count > 0 && !matches!(segment.topology, DeviceTopologyHint::Display { .. })
        })
        .cloned()
        .collect::<Vec<_>>();
    // A device that reports no light segments has said nothing about its
    // layout yet: a hub or radio learns its topology at connect, and until
    // then a rescan describes it from the bare descriptor. Pruning on that
    // would throw away every placed output on each daemon start.
    if eligible_segments.is_empty() {
        return 0;
    }
    let expected_segment_names = eligible_segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<HashSet<_>>();
    let before_len = layout.zones.len();
    layout.zones.retain(|zone| {
        if zone.device_id != layout_device_id || !zone.id.starts_with(&auto_zone_prefix) {
            return true;
        }

        zone.zone_name.as_deref().is_some_and(|zone_name| {
            expected_segment_names
                .iter()
                .any(|expected| channel_name_matches_slot_alias(Some(zone_name), Some(expected)))
        })
    });

    let mut repaired = before_len.saturating_sub(layout.zones.len());

    for (index, segment_info) in eligible_segments.iter().enumerate() {
        let layout_hint = segment_info.layout_hint.as_ref();
        let expected_topology = layout_hint
            .and_then(|hint| hint.topology.clone())
            .unwrap_or_else(|| spatial_topology_for_segment(segment_info));

        let (_, default_size) = auto_layout_geometry(
            NormalizedPosition::new(0.5, 0.5),
            index,
            eligible_segments.len(),
            &segment_info.topology,
        );
        let expected_size = layout_hint
            .and_then(|hint| hint.size)
            .unwrap_or(default_size);
        let expected_name = if eligible_segments.len() == 1 {
            device_info.name.clone()
        } else {
            format!("{}: {}", device_info.name, segment_info.name)
        };
        let expected_positions = generate_positions(&expected_topology);
        let expected_shape = layout_hint
            .and_then(|hint| hint.shape.clone())
            .or_else(|| auto_layout_shape(&segment_info.topology));

        for zone in layout.zones.iter_mut().filter(|zone| {
            zone.device_id == layout_device_id
                && zone.zone_name.as_deref().is_some_and(|zone_name| {
                    channel_name_matches_slot_alias(
                        Some(zone_name),
                        Some(segment_info.name.as_str()),
                    )
                })
                && zone.id.starts_with(&auto_zone_prefix)
        }) {
            let mut changed = false;

            // A newly discovered panel outline must inherit physical aspect,
            // while keeping the area and center of an existing placement.
            if zone.shape != expected_shape
                && let Some(physical) = layout_hint.and_then(|hint| hint.placement)
                && let Some(fitted) = SegmentPlacement::fit_all(
                    &[Some(physical)],
                    zone.position,
                    zone.size,
                    (layout.canvas_width, layout.canvas_height),
                )[0]
            {
                let area_scale =
                    (zone.size.x * zone.size.y / (fitted.size.x * fitted.size.y)).sqrt();
                zone.size =
                    NormalizedPosition::new(fitted.size.x * area_scale, fitted.size.y * area_scale);
                changed = true;
            }

            if zone.zone_name.as_deref() != Some(segment_info.name.as_str()) {
                zone.zone_name = Some(segment_info.name.clone());
                changed = true;
            }
            if zone.name != expected_name {
                zone.name.clone_from(&expected_name);
                changed = true;
            }
            if zone.topology != expected_topology {
                zone.topology = expected_topology.clone();
                changed = true;
            }
            if zone.led_positions != expected_positions {
                zone.led_positions.clone_from(&expected_positions);
                changed = true;
            }
            if zone.shape != expected_shape {
                zone.shape.clone_from(&expected_shape);
                changed = true;
            }
            // Physical assemblies are fitted together on insertion. Their
            // canvas transform subsequently belongs to the user's layout.
            if layout_hint.is_none_or(|hint| hint.placement.is_none()) && zone.size != expected_size
            {
                zone.size = expected_size;
                changed = true;
            }

            if changed {
                repaired = repaired.saturating_add(1);
            }
        }
    }

    repaired
}

fn auto_layout_slot_center(slot_index: usize) -> NormalizedPosition {
    const COLUMNS: usize = 3;
    const LEFT_X: f32 = 0.18;
    const TOP_Y: f32 = 0.18;
    const X_SPACING: f32 = 0.32;
    const Y_SPACING: f32 = 0.22;
    let column = slot_index % COLUMNS;
    let row = slot_index / COLUMNS;

    let column_f32 = f32::from(u16::try_from(column).unwrap_or(u16::MAX));
    let row_f32 = f32::from(u16::try_from(row).unwrap_or(u16::MAX));
    NormalizedPosition::new(
        (LEFT_X + X_SPACING * column_f32).clamp(0.12, 0.88),
        (TOP_Y + Y_SPACING * row_f32).clamp(0.14, 0.86),
    )
}

fn auto_layout_geometry(
    slot_center: NormalizedPosition,
    zone_index: usize,
    zone_count: usize,
    topology: &DeviceTopologyHint,
) -> (NormalizedPosition, NormalizedPosition) {
    let slot_width = 0.26;
    let slot_height = 0.18;
    let zone_count_f32 = f32::from(u16::try_from(zone_count.max(1)).unwrap_or(u16::MAX));
    let zone_index_f32 = f32::from(u16::try_from(zone_index).unwrap_or(u16::MAX));
    let steps = zone_count.saturating_sub(1);
    let steps_f32 = f32::from(u16::try_from(steps).unwrap_or(u16::MAX));
    let step = if zone_count <= 1 {
        0.0
    } else {
        (slot_height / zone_count_f32).min(0.08)
    };
    let offset = if zone_count <= 1 {
        0.0
    } else {
        -step * steps_f32 / 2.0 + step * zone_index_f32
    };
    let position = NormalizedPosition::new(slot_center.x, (slot_center.y + offset).clamp(0.1, 0.9));

    let size = match topology {
        DeviceTopologyHint::Strip | DeviceTopologyHint::Custom => {
            NormalizedPosition::new(slot_width, (slot_height / zone_count_f32).clamp(0.05, 0.1))
        }
        DeviceTopologyHint::Matrix { rows, cols } => {
            let rows_f32 = f32::from(u16::try_from(*rows).unwrap_or(u16::MAX));
            let cols_f32 = f32::from(u16::try_from(*cols).unwrap_or(u16::MAX));
            let aspect = if rows_f32 <= 0.0 {
                1.0
            } else {
                cols_f32 / rows_f32
            };
            let width = 0.18_f32.clamp(0.12, slot_width);
            // Dense multi-zone devices cannot always preserve the preferred minimum matrix height.
            let max_height = slot_height / zone_count_f32;
            let min_height = max_height.min(0.08);
            let height = (width / aspect).clamp(min_height, max_height);
            NormalizedPosition::new(width, height)
        }
        DeviceTopologyHint::Ring { .. } => {
            let diameter = (0.16 / zone_count_f32.max(1.0)).clamp(0.08, 0.16);
            NormalizedPosition::new(diameter, diameter)
        }
        DeviceTopologyHint::Point => NormalizedPosition::new(0.08, 0.08),
        DeviceTopologyHint::Display { .. } => NormalizedPosition::new(0.18, 0.12),
    };

    (position, size)
}

fn spatial_topology_for_segment(
    segment_info: &hypercolor_types::device::SegmentInfo,
) -> LedTopology {
    match segment_info.topology {
        DeviceTopologyHint::Strip
        | DeviceTopologyHint::Custom
        | DeviceTopologyHint::Display { .. } => LedTopology::Strip {
            count: segment_info.led_count,
            direction: StripDirection::LeftToRight,
        },
        DeviceTopologyHint::Matrix { rows, cols } => LedTopology::Matrix {
            width: cols,
            height: rows,
            serpentine: false,
            start_corner: Corner::TopLeft,
        },
        DeviceTopologyHint::Ring { count } => LedTopology::Ring {
            count,
            start_angle: 0.0,
            direction: Winding::Clockwise,
        },
        DeviceTopologyHint::Point => LedTopology::Point,
    }
}

fn auto_layout_shape(topology: &DeviceTopologyHint) -> Option<ZoneShape> {
    match topology {
        DeviceTopologyHint::Ring { .. } => Some(ZoneShape::Ring),
        DeviceTopologyHint::Point => None,
        DeviceTopologyHint::Strip
        | DeviceTopologyHint::Matrix { .. }
        | DeviceTopologyHint::Custom
        | DeviceTopologyHint::Display { .. } => Some(ZoneShape::Rectangle),
    }
}

fn unique_auto_zone_id(layout: &SpatialLayout, layout_device_id: &str, zone_name: &str) -> String {
    let device_component = sanitize_auto_layout_component(layout_device_id);
    let zone_component = sanitize_auto_layout_component(zone_name);
    let base = format!("auto-{device_component}-{zone_component}");
    if !layout.zones.iter().any(|zone| zone.id == base) {
        return base;
    }

    let mut suffix = 2_u32;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !layout.zones.iter().any(|zone| zone.id == candidate) {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

fn sanitize_auto_layout_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_was_dash = false;
    for ch in raw.chars() {
        let normalized = if ch.is_ascii_alphanumeric() {
            Some(ch.to_ascii_lowercase())
        } else if ch == '-' || ch == '_' || ch == ':' || ch.is_ascii_whitespace() {
            Some('-')
        } else {
            None
        };

        let Some(ch) = normalized else {
            continue;
        };

        if ch == '-' {
            if prev_was_dash || out.is_empty() {
                continue;
            }
            prev_was_dash = true;
            out.push(ch);
            continue;
        }

        prev_was_dash = false;
        out.push(ch);
    }

    while out.ends_with('-') {
        out.pop();
    }

    if out.is_empty() {
        "zone".to_owned()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_types::device::{
        ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceId, DeviceInfo,
        DeviceOrigin, DeviceTopologyHint, DisplayFrameFormat, SegmentInfo, SegmentLayoutHint,
        SegmentPlacement,
    };
    use hypercolor_types::spatial::{
        EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout, ZoneShape,
    };

    use super::{
        adopt_primary_zone_outputs, append_auto_layout_zones_for_device,
        append_screen_outputs_for_device, reconcile_auto_layout_zones_for_device,
    };
    use hypercolor_types::scene::{Zone, ZoneRole};

    fn layout() -> SpatialLayout {
        SpatialLayout {
            id: "default".to_owned(),
            name: "Default Layout".to_owned(),
            description: None,
            canvas_width: 320,
            canvas_height: 200,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            version: 1,
        }
    }

    fn device(segments: Vec<SegmentInfo>) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new(),
            name: "Desk Device".to_owned(),
            vendor: "Test".to_owned(),
            family: DeviceFamily::new_static("layout-test", "Layout Test"),
            model: None,
            connection_type: ConnectionType::Usb,
            origin: DeviceOrigin::native("layout-test", "usb", ConnectionType::Usb),
            segments,
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        }
    }

    fn segment(name: &str, led_count: u32, topology: DeviceTopologyHint) -> SegmentInfo {
        SegmentInfo {
            name: name.to_owned(),
            led_count,
            topology,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }
    }

    #[test]
    fn append_mints_addressable_segments_and_skips_display_surfaces() {
        let info = device(vec![
            segment("Main", 30, DeviceTopologyHint::Strip),
            segment(
                "Screen",
                1,
                DeviceTopologyHint::Display {
                    width: 320,
                    height: 320,
                    circular: true,
                    format: DisplayFrameFormat::Jpeg,
                },
            ),
        ]);
        let mut layout = layout();

        assert_eq!(
            append_auto_layout_zones_for_device(&mut layout, "usb:desk", &info),
            1
        );
        assert_eq!(layout.zones.len(), 1);
        assert_eq!(layout.zones[0].device_id, "usb:desk");
        assert_eq!(layout.zones[0].zone_name.as_deref(), Some("Main"));
        assert_eq!(
            layout.zones[0].topology,
            LedTopology::Strip {
                count: 30,
                direction: hypercolor_types::spatial::StripDirection::LeftToRight,
            }
        );
    }

    #[test]
    fn append_skips_display_only_devices() {
        let info = device(vec![segment(
            "Screen",
            1,
            DeviceTopologyHint::Display {
                width: 320,
                height: 320,
                circular: true,
                format: DisplayFrameFormat::Jpeg,
            },
        )]);
        let mut layout = layout();

        assert_eq!(
            append_auto_layout_zones_for_device(&mut layout, "usb:display", &info),
            0
        );
        assert!(layout.zones.is_empty());
    }

    #[test]
    fn a_screen_gets_a_box_only_when_it_is_asked_for() {
        // Automatic layout leaves a screen off the canvas. An assignment
        // is someone placing it, and mints a box of the panel's shape.
        let panel = segment(
            "Display",
            0,
            DeviceTopologyHint::Display {
                width: 960,
                height: 160,
                circular: false,
                format: DisplayFrameFormat::Jpeg,
            },
        );
        let info = device(vec![
            segment("Ring", 12, DeviceTopologyHint::Ring { count: 12 }),
            panel,
        ]);
        let mut layout = layout();
        assert_eq!(
            append_auto_layout_zones_for_device(&mut layout, "usb:pump", &info),
            1
        );
        let ring = layout.zones[0].position;

        assert_eq!(
            append_screen_outputs_for_device(&mut layout, "usb:pump", &info),
            1
        );
        let screen = &layout.zones[1];
        assert_eq!(screen.zone_name.as_deref(), Some("Display"));
        assert_eq!(screen.device_id, "usb:pump");
        assert!(screen.name.ends_with(": Display"));
        // It sits on the device's lights, six times as wide as tall on
        // the canvas whatever the canvas's own shape.
        assert_eq!(screen.position, ring);
        let width = screen.size.x * f32::from(u16::try_from(layout.canvas_width).expect("fits"));
        let height = screen.size.y * f32::from(u16::try_from(layout.canvas_height).expect("fits"));
        assert!(
            (width / height - 6.0).abs() < 0.01,
            "aspect was {}",
            width / height
        );
        assert!(screen.led_positions.is_empty());
        assert_ne!(screen.id, layout.zones[0].id);

        // A device that is a screen and nothing else takes the device's
        // name and the middle of the canvas.
        let lone = device(vec![segment(
            "Display",
            0,
            DeviceTopologyHint::Display {
                width: 320,
                height: 320,
                circular: true,
                format: DisplayFrameFormat::Jpeg,
            },
        )]);
        let mut empty = self::layout();
        assert_eq!(
            append_screen_outputs_for_device(&mut empty, "usb:lcd", &lone),
            1
        );
        assert_eq!(empty.zones[0].name, lone.name);
        assert_eq!(empty.zones[0].position, NormalizedPosition::new(0.5, 0.5));
    }

    #[test]
    fn append_honors_declared_custom_geometry() {
        let hint =
            SegmentLayoutHint::custom_grid(3, 2, &[(0, 0), (1, 0), (2, 0), (2, 1), (1, 1), (0, 1)])
                .with_size(NormalizedPosition::new(0.2, 0.08))
                .with_shape(ZoneShape::Rectangle);
        let mut custom = segment("Perimeter", 6, DeviceTopologyHint::Custom);
        custom.layout_hint = Some(hint);
        let info = device(vec![custom]);
        let mut layout = layout();

        assert_eq!(
            append_auto_layout_zones_for_device(&mut layout, "usb:custom", &info),
            1
        );
        assert_eq!(layout.zones[0].size, NormalizedPosition::new(0.2, 0.08));
        assert_eq!(layout.zones[0].shape, Some(ZoneShape::Rectangle));
        match &layout.zones[0].topology {
            LedTopology::Custom { positions } => assert_eq!(positions.len(), 6),
            other => panic!("expected custom topology, got {other:?}"),
        }
    }

    #[test]
    fn append_preserves_colocated_ring_geometry() {
        let outer = SegmentLayoutHint::custom_grid(3, 3, &[(2, 1), (1, 2), (0, 1), (1, 0)])
            .with_size(NormalizedPosition::new(0.2, 0.2))
            .with_shape(ZoneShape::Ring)
            .co_located();
        let inner = SegmentLayoutHint::custom_grid(3, 3, &[(2, 1), (1, 2), (0, 1), (1, 0)])
            .with_size(NormalizedPosition::new(0.12, 0.12))
            .with_shape(ZoneShape::Ring)
            .co_located();
        let mut outer_segment = segment("Outer", 4, DeviceTopologyHint::Ring { count: 4 });
        outer_segment.layout_hint = Some(outer);
        let mut inner_segment = segment("Inner", 4, DeviceTopologyHint::Ring { count: 4 });
        inner_segment.layout_hint = Some(inner);
        let info = device(vec![outer_segment, inner_segment]);
        let mut layout = layout();

        assert_eq!(
            append_auto_layout_zones_for_device(&mut layout, "usb:rings", &info),
            2
        );
        assert_eq!(layout.zones[0].position, layout.zones[1].position);
        assert_eq!(layout.zones[0].size, NormalizedPosition::new(0.2, 0.2));
        assert_eq!(layout.zones[1].size, NormalizedPosition::new(0.12, 0.12));
    }

    #[test]
    fn append_preserves_asymmetric_custom_position_order() {
        let positions = &[(2, 3), (2, 0), (0, 1), (4, 1), (3, 2)];
        let mut custom = segment("Pointer", 5, DeviceTopologyHint::Custom);
        custom.layout_hint = Some(SegmentLayoutHint::custom_grid(5, 4, positions));
        let info = device(vec![custom]);
        let mut layout = layout();

        let _ = append_auto_layout_zones_for_device(&mut layout, "usb:pointer", &info);
        let LedTopology::Custom { positions: actual } = &layout.zones[0].topology else {
            panic!("expected custom topology");
        };
        assert_eq!(actual.len(), positions.len());
        assert!(actual[0].y > actual[1].y);
        assert!(actual[2].x < actual[3].x);
    }

    #[test]
    fn append_dense_matrix_clamps_geometry_to_normalized_space() {
        let info = device(
            (0..12)
                .map(|index| {
                    segment(
                        &format!("Matrix {index}"),
                        64,
                        DeviceTopologyHint::Matrix { rows: 8, cols: 8 },
                    )
                })
                .collect(),
        );
        let mut layout = layout();

        assert_eq!(
            append_auto_layout_zones_for_device(&mut layout, "usb:matrix", &info),
            12
        );
        assert!(layout.zones.iter().all(|zone| {
            zone.position.x >= 0.0
                && zone.position.x <= 1.0
                && zone.position.y >= 0.0
                && zone.position.y <= 1.0
                && zone.size.x >= 0.0
                && zone.size.x <= 1.0
                && zone.size.y >= 0.0
                && zone.size.y <= 1.0
        }));
    }

    #[test]
    fn reconcile_repairs_geometry_preserves_authored_rotation_and_removes_stale_zones() {
        let mut original = segment("Main", 10, DeviceTopologyHint::Strip);
        let mut stale = segment("Removed", 5, DeviceTopologyHint::Strip);
        let initial = device(vec![original.clone(), stale.clone()]);
        let mut layout = layout();
        assert_eq!(
            append_auto_layout_zones_for_device(&mut layout, "usb:repair", &initial),
            2
        );
        layout.zones[0].rotation = 37.0;
        original.led_count = 24;
        original.layout_hint = Some(
            SegmentLayoutHint::custom_grid(2, 2, &[(0, 0), (1, 0), (1, 1), (0, 1)])
                .with_size(NormalizedPosition::new(0.3, 0.2)),
        );
        stale.name = "Unused".to_owned();
        let updated = device(vec![original]);

        assert_eq!(
            reconcile_auto_layout_zones_for_device(&mut layout, "usb:repair", &updated),
            2
        );
        assert_eq!(layout.zones.len(), 1);
        assert_eq!(layout.zones[0].rotation, 37.0);
        assert_eq!(layout.zones[0].size, NormalizedPosition::new(0.3, 0.2));
        match &layout.zones[0].topology {
            LedTopology::Custom { positions } => assert_eq!(positions.len(), 4),
            other => panic!("expected repaired custom topology, got {other:?}"),
        }
    }

    #[test]
    fn physical_outline_upgrade_preserves_authored_area_and_subsequent_size_edits() {
        let mut panel = segment("Panel 1", 1, DeviceTopologyHint::Point);
        let mut layout = layout();
        let initial = device(vec![panel.clone()]);
        assert_eq!(
            append_auto_layout_zones_for_device(&mut layout, "net:panels", &initial),
            1
        );
        let position = NormalizedPosition::new(0.72, 0.61);
        let old_size = NormalizedPosition::new(0.2, 0.15);
        layout.zones[0].position = position;
        layout.zones[0].size = old_size;
        layout.zones[0].rotation = 37.0;
        layout.zones[0].scale = 1.25;
        assert!(layout.zones[0].shape.is_none());
        let shape = ZoneShape::Custom {
            vertices: vec![
                NormalizedPosition::new(0.5, 0.0),
                NormalizedPosition::new(0.0, 1.0),
                NormalizedPosition::new(1.0, 1.0),
            ],
        };
        let centroid = NormalizedPosition::new(0.5, 2.0 / 3.0);
        panel.layout_hint = Some(SegmentLayoutHint {
            topology: Some(LedTopology::Custom {
                positions: vec![centroid],
            }),
            shape: Some(shape.clone()),
            placement: Some(SegmentPlacement {
                position: NormalizedPosition::new(0.0, 0.0),
                size: NormalizedPosition::new(150.0, 75.0 * 3.0_f32.sqrt()),
                rotation: 0.0,
            }),
            ..SegmentLayoutHint::default()
        });
        let upgraded = device(vec![panel]);
        assert_eq!(
            reconcile_auto_layout_zones_for_device(&mut layout, "net:panels", &upgraded),
            1
        );
        let zone = &layout.zones[0];
        assert_eq!(zone.position, position);
        assert_eq!(zone.rotation, 37.0);
        assert_eq!(zone.scale, 1.25);
        assert_eq!(zone.shape, Some(shape));
        assert_eq!(zone.led_positions, vec![centroid]);
        assert!((zone.size.x * zone.size.y - old_size.x * old_size.y).abs() < 0.000_001);
        let pixel_aspect =
            zone.size.y * layout.canvas_height as f32 / (zone.size.x * layout.canvas_width as f32);
        assert!((pixel_aspect - 3.0_f32.sqrt() / 2.0).abs() < 0.000_001);

        let edited_size = NormalizedPosition::new(0.31, 0.07);
        layout.zones[0].size = edited_size;
        assert_eq!(
            reconcile_auto_layout_zones_for_device(&mut layout, "net:panels", &upgraded),
            0
        );
        assert_eq!(layout.zones[0].size, edited_size);
        assert_eq!(layout.zones[0].position, position);
        assert_eq!(layout.zones[0].rotation, 37.0);
    }

    #[test]
    fn reconcile_updates_an_existing_auto_zone_without_duplicating_it() {
        let initial = device(vec![segment("Main", 10, DeviceTopologyHint::Strip)]);
        let mut layout = layout();
        let _ = append_auto_layout_zones_for_device(&mut layout, "usb:update", &initial);
        let mut updated_segment = segment("Main", 20, DeviceTopologyHint::Strip);
        updated_segment.layout_hint = Some(
            SegmentLayoutHint::custom_grid(2, 2, &[(0, 0), (1, 0), (1, 1), (0, 1)])
                .with_size(NormalizedPosition::new(0.25, 0.25)),
        );
        let updated = device(vec![updated_segment]);

        assert_eq!(
            reconcile_auto_layout_zones_for_device(&mut layout, "usb:update", &updated),
            1
        );
        assert_eq!(layout.zones.len(), 1);
        assert_eq!(layout.zones[0].zone_name.as_deref(), Some("Main"));
        assert_eq!(layout.zones[0].size, NormalizedPosition::new(0.25, 0.25));
    }

    #[test]
    fn reconcile_removes_only_stale_auto_zones() {
        let initial = device(vec![
            segment("Main", 10, DeviceTopologyHint::Strip),
            segment("Aux", 5, DeviceTopologyHint::Strip),
        ]);
        let mut layout = layout();
        let _ = append_auto_layout_zones_for_device(&mut layout, "usb:remove", &initial);
        let authored = layout.zones[0].clone();
        let mut authored = hypercolor_types::spatial::Output {
            id: "authored-zone".to_owned(),
            ..authored
        };
        authored.device_id = "usb:remove".to_owned();
        layout.zones.push(authored);
        let updated = device(vec![segment("Main", 10, DeviceTopologyHint::Strip)]);

        assert_eq!(
            reconcile_auto_layout_zones_for_device(&mut layout, "usb:remove", &updated),
            2
        );
        assert_eq!(layout.zones.len(), 2);
        assert!(layout.zones.iter().any(|zone| zone.id == "authored-zone"));
        assert!(
            layout
                .zones
                .iter()
                .all(|zone| zone.zone_name.as_deref() != Some("Aux"))
        );
    }

    /// A hub rediscovered before it connects reports no segments; its
    /// placed outputs must survive that, or every restart empties them.
    #[test]
    fn reconcile_keeps_placed_zones_while_the_device_reports_no_segments() {
        let mut layout = layout();
        let initial = device(vec![segment(
            "Fan 1",
            26,
            DeviceTopologyHint::Ring { count: 26 },
        )]);
        let _ = append_auto_layout_zones_for_device(&mut layout, "usb:radio", &initial);
        assert_eq!(layout.zones.len(), 1);

        let rescanned = device(Vec::new());
        assert_eq!(
            reconcile_auto_layout_zones_for_device(&mut layout, "usb:radio", &rescanned),
            0
        );
        assert_eq!(
            layout.zones.len(),
            1,
            "the ring waits for the device to connect"
        );
    }

    /// Studio mints a member into the scene's primary zone; the layout store
    /// hears about it at restore and must not throw it away.
    #[test]
    fn restore_adopts_outputs_the_primary_zone_holds_and_the_layout_lacks() {
        let mut stored = layout();
        let radio = device(vec![segment(
            "Fan 1",
            26,
            DeviceTopologyHint::Ring { count: 26 },
        )]);
        let mut held = layout();
        let _ = append_auto_layout_zones_for_device(&mut held, "usb:radio", &radio);
        let zone = |role: ZoneRole, layout: SpatialLayout| Zone {
            id: hypercolor_types::scene::ZoneId::new(),
            name: "Zone".to_owned(),
            description: None,
            layers: Vec::new(),
            layout,
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: None,
            role,
            controls_version: 0,
            layers_version: 0,
        };
        let primary = zone(ZoneRole::Primary, held.clone());
        let screen = zone(ZoneRole::Display, held);

        assert_eq!(adopt_primary_zone_outputs(&mut stored, &[screen]), 0);
        assert_eq!(
            adopt_primary_zone_outputs(&mut stored, std::slice::from_ref(&primary)),
            1
        );
        assert_eq!(stored.zones.len(), 1);
        assert_eq!(
            adopt_primary_zone_outputs(&mut stored, &[primary]),
            0,
            "an output already in the layout is not adopted twice"
        );
    }
}
