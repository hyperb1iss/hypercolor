use std::collections::{HashMap, HashSet};

use hypercolor_core::device::DeviceLifecycleManager;
use hypercolor_core::spatial::generate_positions;
use hypercolor_types::device::{DeviceId, DeviceInfo, DeviceTopologyHint};
use hypercolor_types::spatial::{
    Corner, DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection, Winding, ZoneShape,
};
use tracing::{debug, info, warn};

use super::DiscoveryRuntime;
use crate::scene_transactions::apply_layout_update;

#[doc(hidden)]
#[allow(
    clippy::too_many_lines,
    reason = "layout reconciliation keeps the full discovery-driven repair flow in one place"
)]
pub async fn sync_active_layout_for_renderable_devices(
    runtime: &DiscoveryRuntime,
    limit_to_devices: Option<&HashSet<DeviceId>>,
) {
    let mut layout = {
        let spatial = runtime.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };
    let excluded_layout_device_ids = {
        let store = runtime.layout_auto_exclusions.read().await;
        store.get(&layout.id).cloned().unwrap_or_default()
    };

    let inactive_ids = {
        let manager = runtime.backend_manager.lock().await;
        manager
            .connected_devices_without_layout_targets(&layout)
            .into_iter()
            .map(|(_, device_id)| device_id)
            .collect::<HashSet<_>>()
    };

    let tracked_devices = runtime.device_registry.list().await;
    let logical_store = runtime.logical_devices.read().await.clone();
    let canonical_layout_ids = {
        let lifecycle = runtime.lifecycle_manager.lock().await;
        tracked_devices
            .iter()
            .map(|tracked| {
                let device_id = tracked.info.id;
                let layout_id = lifecycle
                    .layout_device_id_for(device_id)
                    .map(ToOwned::to_owned);
                (device_id, layout_id)
            })
            .collect::<HashMap<_, _>>()
    };

    let mut repaired_devices = Vec::new();
    let mut repaired_zone_count = 0_usize;
    for tracked in tracked_devices {
        let device_id = tracked.info.id;
        if !tracked.state.is_renderable() {
            continue;
        }
        if limit_to_devices.is_some_and(|allowed| !allowed.contains(&device_id)) {
            continue;
        }

        let layout_device_id =
            if let Some(Some(layout_device_id)) = canonical_layout_ids.get(&device_id) {
                layout_device_id.clone()
            } else {
                let fingerprint = runtime.device_registry.fingerprint_for_id(&device_id).await;
                let backend_id = tracked.info.output_backend_id();
                DeviceLifecycleManager::canonical_layout_device_id(
                    backend_id,
                    &tracked.info,
                    fingerprint.as_ref(),
                )
            };
        let default_enabled = logical_store
            .get(&layout_device_id)
            .is_none_or(|entry| entry.enabled);
        if !default_enabled {
            if inactive_ids.contains(&device_id) {
                debug!(
                    device_id = %device_id,
                    device_name = %tracked.info.name,
                    layout_device_id = %layout_device_id,
                    "skipping auto-layout sync because the default logical device is disabled"
                );
            }
            continue;
        }
        if excluded_layout_device_ids.contains(&layout_device_id) {
            if inactive_ids.contains(&device_id) {
                debug!(
                    device_id = %device_id,
                    device_name = %tracked.info.name,
                    layout_device_id = %layout_device_id,
                    layout_id = %layout.id,
                    "skipping auto-layout sync because the device is excluded from the active layout"
                );
            }
            continue;
        }

        let repaired =
            reconcile_auto_layout_zones_for_device(&mut layout, &layout_device_id, &tracked.info);
        if repaired > 0 {
            repaired_zone_count = repaired_zone_count.saturating_add(repaired);
            repaired_devices.push(format!("{} ({device_id})", tracked.info.name));
        }

        if inactive_ids.contains(&device_id) {
            debug!(
                device_id = %device_id,
                device_name = %tracked.info.name,
                layout_device_id = %layout_device_id,
                zone_count = tracked.info.zones.len(),
                total_leds = tracked.info.total_led_count(),
                "leaving layout-inactive device out of the active layout until it is explicitly mapped"
            );
        }
    }

    if repaired_devices.is_empty() {
        return;
    }

    {
        apply_layout_update(
            &runtime.spatial_engine,
            &runtime.scene_manager,
            &runtime.scene_transactions,
            layout.clone(),
        )
        .await;
    }

    let layouts_snapshot = {
        let mut layouts = runtime.layouts.write().await;
        layouts.insert(layout.id.clone(), layout.clone());
        layouts.clone()
    };
    if let Err(error) = crate::layout_store::save(&runtime.layouts_path, &layouts_snapshot) {
        warn!(
            path = %runtime.layouts_path.display(),
            %error,
            "failed to persist auto-updated layout store"
        );
    }

    info!(
        layout_id = %layout.id,
        layout_name = %layout.name,
        repaired_device_count = repaired_devices.len(),
        repaired_zone_count,
        repaired_devices = ?repaired_devices,
        "reconciled existing auto-layout zones in the active layout"
    );
}

#[doc(hidden)]
pub async fn sync_active_layout_connectivity(
    runtime: &DiscoveryRuntime,
    limit_to_devices: Option<&HashSet<DeviceId>>,
) {
    let tracked_devices = runtime.device_registry.list().await;

    for tracked in tracked_devices {
        let device_id = tracked.info.id;
        if limit_to_devices.is_some_and(|allowed| !allowed.contains(&device_id)) {
            continue;
        }

        let backend = tracked.info.output_backend_id();
        let fingerprint = runtime.device_registry.fingerprint_for_id(&device_id).await;
        let connect_behavior = super::device_helpers::desired_connect_behavior(
            runtime,
            device_id,
            &tracked.info,
            backend,
            fingerprint.as_ref(),
            tracked.connect_behavior,
            tracked.user_settings.enabled,
        )
        .await;

        let actions = {
            let mut lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle.on_discovered_with_behavior(
                device_id,
                &tracked.info,
                &backend,
                fingerprint.as_ref(),
                connect_behavior,
            )
        };
        if actions.is_empty() {
            continue;
        }

        super::lifecycle::execute_lifecycle_actions(runtime.clone(), actions).await;
        super::device_helpers::sync_registry_state(runtime, device_id).await;
    }

    sync_active_layout_for_renderable_devices(runtime, limit_to_devices).await;
}

#[doc(hidden)]
#[must_use]
pub fn append_auto_layout_zones_for_device(
    layout: &mut SpatialLayout,
    layout_device_id: &str,
    device_info: &DeviceInfo,
) -> usize {
    let eligible_zones = device_info
        .zones
        .iter()
        .filter(|zone| {
            zone.led_count > 0 && !matches!(zone.topology, DeviceTopologyHint::Display { .. })
        })
        .cloned()
        .collect::<Vec<_>>();
    if eligible_zones.is_empty() {
        return 0;
    }

    let existing_device_count = layout
        .zones
        .iter()
        .map(|zone| zone.device_id.as_str())
        .collect::<HashSet<_>>()
        .len();
    let slot_center = auto_layout_slot_center(existing_device_count);

    for (index, zone_info) in eligible_zones.iter().enumerate() {
        let override_spec = auto_layout_override(device_info, zone_info);
        let topology = override_spec.as_ref().map_or_else(
            || spatial_topology_for_zone(zone_info),
            |spec| spec.topology.clone(),
        );
        let (default_position, default_size) = auto_layout_geometry(
            slot_center,
            index,
            eligible_zones.len(),
            &zone_info.topology,
        );
        let position = override_spec
            .as_ref()
            .filter(|spec| spec.co_located)
            .map_or(default_position, |_| slot_center);
        let size = override_spec
            .as_ref()
            .and_then(|spec| spec.size)
            .unwrap_or(default_size);
        let zone_id = unique_auto_zone_id(layout, layout_device_id, &zone_info.name);
        let zone_name = if eligible_zones.len() == 1 {
            device_info.name.clone()
        } else {
            format!("{}: {}", device_info.name, zone_info.name)
        };

        layout.zones.push(DeviceZone {
            id: zone_id,
            name: zone_name,
            device_id: layout_device_id.to_owned(),
            zone_name: Some(zone_info.name.clone()),
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
            shape: override_spec
                .as_ref()
                .and_then(|spec| spec.shape.clone())
                .or_else(|| auto_layout_shape(&zone_info.topology)),
            shape_preset: None,
            attachment: None,
            brightness: None,
        });
    }

    eligible_zones.len()
}

#[doc(hidden)]
#[must_use]
pub fn reconcile_auto_layout_zones_for_device(
    layout: &mut SpatialLayout,
    layout_device_id: &str,
    device_info: &DeviceInfo,
) -> usize {
    let auto_zone_prefix = format!("auto-{}-", sanitize_auto_layout_component(layout_device_id));
    let eligible_zones = device_info
        .zones
        .iter()
        .filter(|zone| {
            zone.led_count > 0 && !matches!(zone.topology, DeviceTopologyHint::Display { .. })
        })
        .cloned()
        .collect::<Vec<_>>();
    let expected_zone_names = eligible_zones
        .iter()
        .map(|zone| zone.name.as_str())
        .collect::<HashSet<_>>();
    let before_len = layout.zones.len();
    layout.zones.retain(|zone| {
        if zone.device_id != layout_device_id || !zone.id.starts_with(&auto_zone_prefix) {
            return true;
        }

        zone.zone_name
            .as_deref()
            .is_some_and(|zone_name| expected_zone_names.contains(zone_name))
    });

    let mut repaired = before_len.saturating_sub(layout.zones.len());
    if eligible_zones.is_empty() {
        return repaired;
    }

    for (index, zone_info) in eligible_zones.iter().enumerate() {
        let override_spec = auto_layout_override(device_info, zone_info);
        let expected_topology = override_spec.as_ref().map_or_else(
            || spatial_topology_for_zone(zone_info),
            |spec| spec.topology.clone(),
        );

        let (_, default_size) = auto_layout_geometry(
            NormalizedPosition::new(0.5, 0.5),
            index,
            eligible_zones.len(),
            &zone_info.topology,
        );
        let expected_size = override_spec
            .as_ref()
            .and_then(|spec| spec.size)
            .unwrap_or(default_size);
        let expected_name = if eligible_zones.len() == 1 {
            device_info.name.clone()
        } else {
            format!("{}: {}", device_info.name, zone_info.name)
        };
        let expected_positions = generate_positions(&expected_topology);
        let expected_shape = override_spec
            .as_ref()
            .and_then(|spec| spec.shape.clone())
            .or_else(|| auto_layout_shape(&zone_info.topology));

        for zone in layout.zones.iter_mut().filter(|zone| {
            zone.device_id == layout_device_id
                && zone.zone_name.as_deref() == Some(zone_info.name.as_str())
                && zone.id.starts_with(&auto_zone_prefix)
        }) {
            let mut changed = false;

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

#[derive(Clone)]
struct AutoLayoutOverride {
    topology: LedTopology,
    size: Option<NormalizedPosition>,
    shape: Option<ZoneShape>,
    co_located: bool,
}

fn auto_layout_override(
    device_info: &DeviceInfo,
    zone_info: &hypercolor_types::device::ZoneInfo,
) -> Option<AutoLayoutOverride> {
    let family_id = device_info.family.id();
    let device_name = device_info.name.as_str();

    if family_id == "razer" && device_name.contains("Seiren V3") && zone_info.led_count == 10 {
        return Some(AutoLayoutOverride {
            topology: LedTopology::Custom {
                positions: normalized_grid_positions(
                    6,
                    2,
                    &[
                        (1, 0),
                        (2, 0),
                        (3, 0),
                        (4, 0),
                        (0, 1),
                        (1, 1),
                        (2, 1),
                        (3, 1),
                        (4, 1),
                        (5, 1),
                    ],
                ),
            },
            size: Some(NormalizedPosition::new(0.2, 0.08)),
            shape: Some(ZoneShape::Rectangle),
            co_located: false,
        });
    }

    if family_id == "razer" && device_name.contains("Basilisk V3") && zone_info.led_count == 11 {
        return Some(AutoLayoutOverride {
            topology: LedTopology::Custom {
                positions: normalized_grid_positions(
                    7,
                    8,
                    &[
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
                    ],
                ),
            },
            size: Some(NormalizedPosition::new(0.16, 0.18)),
            shape: Some(ZoneShape::Rectangle),
            co_located: false,
        });
    }

    if family_id == "corsair" && zone_info.led_count == 20 && zone_info.name.contains("AIO") {
        return Some(AutoLayoutOverride {
            topology: LedTopology::Custom {
                positions: normalized_grid_positions(
                    13,
                    13,
                    &[
                        (12, 6),
                        (11, 8),
                        (10, 10),
                        (8, 11),
                        (6, 12),
                        (4, 11),
                        (2, 10),
                        (1, 8),
                        (0, 6),
                        (1, 4),
                        (2, 2),
                        (4, 1),
                        (6, 0),
                        (8, 1),
                        (10, 2),
                        (11, 4),
                        (8, 6),
                        (6, 8),
                        (4, 6),
                        (6, 4),
                    ],
                ),
            },
            size: Some(NormalizedPosition::new(0.16, 0.16)),
            shape: Some(ZoneShape::Ring),
            co_located: true,
        });
    }

    if family_id == "corsair"
        && zone_info.led_count == 24
        && zone_info.name.contains("Cooler Pump LCD")
    {
        return Some(AutoLayoutOverride {
            topology: LedTopology::Custom {
                positions: normalized_grid_positions(
                    11,
                    11,
                    &[
                        (10, 5),
                        (9, 6),
                        (9, 7),
                        (8, 8),
                        (7, 9),
                        (6, 9),
                        (5, 10),
                        (4, 9),
                        (3, 9),
                        (2, 8),
                        (1, 7),
                        (1, 6),
                        (0, 5),
                        (1, 4),
                        (1, 3),
                        (2, 2),
                        (3, 1),
                        (4, 1),
                        (5, 0),
                        (6, 1),
                        (7, 1),
                        (8, 2),
                        (9, 3),
                        (9, 4),
                    ],
                ),
            },
            size: Some(NormalizedPosition::new(0.19, 0.19)),
            shape: Some(ZoneShape::Ring),
            co_located: true,
        });
    }

    None
}

fn normalized_grid_positions(
    width: u32,
    height: u32,
    coordinates: &[(u32, u32)],
) -> Vec<NormalizedPosition> {
    let x_divisor = f32::from(u16::try_from(width.saturating_sub(1).max(1)).unwrap_or(u16::MAX));
    let y_divisor = f32::from(u16::try_from(height.saturating_sub(1).max(1)).unwrap_or(u16::MAX));

    coordinates
        .iter()
        .map(|&(x, y)| {
            NormalizedPosition::new(
                f32::from(u16::try_from(x).unwrap_or(u16::MAX)) / x_divisor,
                f32::from(u16::try_from(y).unwrap_or(u16::MAX)) / y_divisor,
            )
        })
        .collect()
}

fn spatial_topology_for_zone(zone_info: &hypercolor_types::device::ZoneInfo) -> LedTopology {
    match zone_info.topology {
        DeviceTopologyHint::Strip
        | DeviceTopologyHint::Custom
        | DeviceTopologyHint::Display { .. } => LedTopology::Strip {
            count: zone_info.led_count,
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
