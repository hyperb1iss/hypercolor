use std::collections::HashSet;

use hypercolor_core::spatial::generate_positions;
use hypercolor_types::attachment::channel_name_matches_slot_alias;
use hypercolor_types::device::{DeviceInfo, DeviceTopologyHint};
use hypercolor_types::spatial::{
    Corner, EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection, Winding, ZoneShape,
};
#[must_use]
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
        let position = layout_hint
            .filter(|hint| hint.co_located)
            .map_or(default_position, |_| slot_center);
        let size = layout_hint
            .and_then(|hint| hint.size)
            .unwrap_or(default_size);
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
            rotation: 0.0,
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
    if eligible_segments.is_empty() {
        return repaired;
    }

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
            if zone.size != expected_size {
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
    };
    use hypercolor_types::spatial::{
        EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout, ZoneShape,
    };

    use super::{append_auto_layout_zones_for_device, reconcile_auto_layout_zones_for_device};

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
}
