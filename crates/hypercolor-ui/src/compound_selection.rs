//! Compound selection — implicit zone grouping derived from device and slot identity.
//!
//! Zones from the same device form a **device compound**. Within a device,
//! attachment zones sharing a `slot_id` form a **slot compound**. The
//! interaction model is Figma-style nesting: click selects a device compound,
//! double-click enters it, Escape exits.

use std::collections::HashSet;

use hypercolor_types::spatial::SpatialLayout;
use serde::{Deserialize, Serialize};

/// Current depth within the compound hierarchy.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum CompoundDepth {
    /// Top level — clicking a zone selects all zones from its device.
    #[default]
    Root,
    /// Entered a specific device — clicking selects slot compounds or individual zones.
    Device { device_id: String },
    /// Entered a specific attachment slot — clicking selects individual zones.
    Slot { device_id: String, slot_id: String },
}

/// All zone IDs belonging to a device compound.
pub(crate) fn device_compound_ids(layout: &SpatialLayout, device_id: &str) -> HashSet<String> {
    layout
        .zones
        .iter()
        .filter(|z| z.device_id == device_id)
        .map(|z| z.id.clone())
        .collect()
}

/// All zone IDs belonging to a slot compound within a device.
pub(crate) fn slot_compound_ids(
    layout: &SpatialLayout,
    device_id: &str,
    slot_id: &str,
) -> HashSet<String> {
    layout
        .zones
        .iter()
        .filter(|z| {
            z.device_id == device_id && z.attachment.as_ref().is_some_and(|a| a.slot_id == slot_id)
        })
        .map(|z| z.id.clone())
        .collect()
}

/// Resolve which zone IDs should be selected when a zone is clicked at the
/// given compound depth.
pub(crate) fn resolve_click(
    layout: &SpatialLayout,
    zone_id: &str,
    depth: &CompoundDepth,
) -> HashSet<String> {
    let Some(zone) = layout.zones.iter().find(|z| z.id == zone_id) else {
        return HashSet::new();
    };

    match depth {
        CompoundDepth::Root => device_compound_ids(layout, &zone.device_id),
        CompoundDepth::Device { device_id } => {
            if zone.device_id != *device_id {
                // Clicked zone outside the entered device — select its device compound
                // and reset to root (caller handles depth change).
                return device_compound_ids(layout, &zone.device_id);
            }
            // Inside the entered device: select slot compound if attachment, else single zone
            zone.attachment
                .as_ref()
                .map(|a| slot_compound_ids(layout, &zone.device_id, &a.slot_id))
                .unwrap_or_else(|| {
                    let mut set = HashSet::new();
                    set.insert(zone_id.to_owned());
                    set
                })
        }
        CompoundDepth::Slot { device_id, slot_id } => {
            if zone.device_id != *device_id {
                return device_compound_ids(layout, &zone.device_id);
            }
            let in_slot = zone
                .attachment
                .as_ref()
                .is_some_and(|a| a.slot_id == *slot_id);
            if !in_slot {
                // Clicked zone outside the entered slot but inside the device
                return zone
                    .attachment
                    .as_ref()
                    .map(|a| slot_compound_ids(layout, &zone.device_id, &a.slot_id))
                    .unwrap_or_else(|| {
                        let mut set = HashSet::new();
                        set.insert(zone_id.to_owned());
                        set
                    });
            }
            // Inside the entered slot: select individual zone
            let mut set = HashSet::new();
            set.insert(zone_id.to_owned());
            set
        }
    }
}
