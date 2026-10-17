//! Keep one display zone per display after a device identity remap.
//!
//! A display that changed physical identity can end up with two display
//! zones once its old identity is remapped: the zone authored against the
//! old identity, and the empty surface minted for the new identity while
//! the old one went unrecognized. Scene validation rejects two display zones
//! for one device, so the remap collapses each such pair to one survivor.

use std::collections::{HashMap, HashSet};

use hypercolor_types::device::DeviceId;
use hypercolor_types::scene::{Scene, UnassignedBehavior, Zone, ZoneId, ZoneRole};
use tracing::info;
use uuid::Uuid;

/// Zones whose display target names an identity the remap replaces.
pub(super) fn retargeted_display_zones(
    zones: &[Zone],
    physical_device_ids: &HashMap<DeviceId, DeviceId>,
) -> HashSet<ZoneId> {
    zones
        .iter()
        .filter(|zone| {
            zone.display_target
                .as_ref()
                .is_some_and(|target| physical_device_ids.contains_key(&target.device_id))
        })
        .map(|zone| zone.id)
        .collect()
}

/// Collapse display zones that now share one display target.
///
/// Only groups containing a retargeted zone are touched, so a collision
/// that predates the remap is left exactly as authored. Returns the number
/// of zones removed.
pub(super) fn collapse_retargeted_display_zones(
    scene: &mut Scene,
    retargeted: &HashSet<ZoneId>,
) -> usize {
    if retargeted.is_empty() {
        return 0;
    }
    let mut groups = HashMap::<DeviceId, Vec<usize>>::new();
    let mut group_order = Vec::new();
    for (index, zone) in scene.zones.iter().enumerate() {
        if zone.role != ZoneRole::Display {
            continue;
        }
        let Some(target) = zone.display_target.as_ref() else {
            continue;
        };
        let group = groups.entry(target.device_id).or_default();
        if group.is_empty() {
            group_order.push(target.device_id);
        }
        group.push(index);
    }

    let mut survivors = HashMap::<ZoneId, ZoneId>::new();
    for device_id in group_order {
        let Some(indices) = groups.get(&device_id) else {
            continue;
        };
        if indices.len() < 2
            || !indices
                .iter()
                .any(|index| retargeted.contains(&scene.zones[*index].id))
        {
            continue;
        }
        let (survivor, reason) = choose_survivor(&scene.zones, indices, retargeted);
        let survivor_id = scene.zones[survivor].id;
        for &index in indices {
            if index == survivor {
                continue;
            }
            let dropped = &scene.zones[index];
            info!(
                scene_id = %scene.id,
                device_id = %device_id,
                kept_zone = %survivor_id,
                kept_layers = scene.zones[survivor].layers.len(),
                dropped_zone = %dropped.id,
                dropped_layers = dropped.layers.len(),
                reason,
                "Collapsed duplicate display zone after device identity remap"
            );
            survivors.insert(dropped.id, survivor_id);
        }
    }
    if survivors.is_empty() {
        return 0;
    }

    scene.zones.retain(|zone| !survivors.contains_key(&zone.id));
    if let UnassignedBehavior::Fallback(zone_id) = &mut scene.unassigned_behavior
        && let Some(survivor) = survivors.get(zone_id)
    {
        *zone_id = *survivor;
    }
    survivors.len()
}

/// Drop retargeted runtime default faces that duplicate a default already
/// registered for the same display.
///
/// Defaults mirror the display preference store, whose identity migration
/// keeps the preference already filed under the current identity, so the
/// existing default wins here too. Returns the number of zones removed.
pub(super) fn drop_retargeted_default_duplicates(
    zones: &mut Vec<Zone>,
    retargeted: &HashSet<ZoneId>,
) -> usize {
    if retargeted.is_empty() {
        return 0;
    }
    let mut claimed = zones
        .iter()
        .filter(|zone| !retargeted.contains(&zone.id))
        .filter_map(|zone| zone.display_target.as_ref().map(|target| target.device_id))
        .collect::<HashSet<_>>();
    let before = zones.len();
    zones.retain(|zone| {
        let Some(target) = zone.display_target.as_ref() else {
            return true;
        };
        !retargeted.contains(&zone.id) || claimed.insert(target.device_id)
    });
    before - zones.len()
}

fn choose_survivor(
    zones: &[Zone],
    indices: &[usize],
    retargeted: &HashSet<ZoneId>,
) -> (usize, &'static str) {
    let prefer_retargeted = |candidates: &[usize]| {
        candidates
            .iter()
            .copied()
            .find(|index| retargeted.contains(&zones[*index].id))
            .unwrap_or(candidates[0])
    };
    let with_layers = indices
        .iter()
        .copied()
        .filter(|index| !zones[*index].layers.is_empty())
        .collect::<Vec<_>>();
    match with_layers.as_slice() {
        [] => (
            prefer_retargeted(indices),
            "no zone carries layers; kept the remapped zone",
        ),
        [only] => (*only, "kept the only zone carrying layers"),
        several => {
            let assigned_at = several
                .iter()
                .map(|index| newest_layer_millis(&zones[*index]))
                .collect::<Option<Vec<_>>>();
            let Some(assigned_at) = assigned_at else {
                return (
                    prefer_retargeted(several),
                    "layer assignment time is unknown; kept the remapped zone",
                );
            };
            let newest = assigned_at.iter().copied().max().unwrap_or_default();
            let latest = several
                .iter()
                .zip(&assigned_at)
                .filter(|(_, millis)| **millis == newest)
                .map(|(index, _)| *index)
                .collect::<Vec<_>>();
            if let [only] = latest.as_slice() {
                (*only, "kept the most recently assigned layers")
            } else {
                (
                    prefer_retargeted(&latest),
                    "layers were assigned at the same time; kept the remapped zone",
                )
            }
        }
    }
}

/// Creation time of the newest layer in the stack, when every layer id is a
/// UUIDv7. Layer ids are minted whenever a layer or face is assigned, so this
/// dates the stack's most recent assignment; control edits do not move it.
fn newest_layer_millis(zone: &Zone) -> Option<u64> {
    zone.layers
        .iter()
        .map(|layer| uuid_v7_millis(layer.id.as_uuid()))
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .max()
}

fn uuid_v7_millis(uuid: Uuid) -> Option<u64> {
    if uuid.get_version_num() != 7 {
        return None;
    }
    u64::try_from(uuid.as_u128() >> 80).ok()
}
