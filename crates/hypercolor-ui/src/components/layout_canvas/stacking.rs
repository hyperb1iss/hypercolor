//! Stacking order for the zone boxes on the Studio canvas.
//!
//! Boxes overlap constantly on a real rig (a fan ring over a strip, a pump
//! cap inside an AIO block), so the order has to be one a user can predict:
//! big boxes sit low so everything inside their footprint stays clickable,
//! anything lifted by selection or hover rises above the rest, the box under
//! the pointer rises above those, and the box that was actually clicked is
//! always on top of its own compound.

use std::collections::HashMap;

use hypercolor_types::spatial::Output;

/// Which lift a box currently earns. Higher tiers win outright; within a
/// tier the area rank breaks ties, so the order stays stable and legible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tier {
    /// The box the pointer last pressed on.
    pub primary: bool,
    /// The box under the pointer on the canvas itself.
    pub under_pointer: bool,
    /// Selected, or previewed from the zone tree.
    pub lifted: bool,
}

const BASE: usize = 10;
const LIFTED: usize = 10_000;
const UNDER_POINTER: usize = 20_000;
const PRIMARY: usize = 30_000;

/// CSS `z-index` for a box. `rank` is its area rank from [`rank_by_area`].
#[must_use]
pub fn z_index(tier: Tier, rank: usize) -> usize {
    let floor = if tier.primary {
        PRIMARY
    } else if tier.under_pointer {
        UNDER_POINTER
    } else if tier.lifted {
        LIFTED
    } else {
        BASE
    };
    floor + rank
}

/// Rank every zone by footprint, largest first (rank 0), so a larger box
/// always stacks beneath a smaller one. Equal areas keep `display_order`,
/// then source order, so the saved order still decides between twins.
#[must_use]
pub fn rank_by_area(zones: &[Output]) -> HashMap<String, usize> {
    let mut ordered: Vec<(usize, &Output)> = zones.iter().enumerate().collect();
    ordered.sort_by(|(ia, a), (ib, b)| {
        footprint(b)
            .total_cmp(&footprint(a))
            .then(a.display_order.cmp(&b.display_order))
            .then(ia.cmp(ib))
    });
    ordered
        .into_iter()
        .enumerate()
        .map(|(rank, (_, zone))| (zone.id.clone(), rank))
        .collect()
}

fn footprint(zone: &Output) -> f32 {
    let scale = zone.scale.max(0.0);
    (zone.size.x * zone.size.y * scale * scale).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypercolor_types::spatial::{LedTopology, NormalizedPosition, StripDirection};

    fn zone(id: &str, w: f32, h: f32, order: i32) -> Output {
        Output {
            id: id.to_owned(),
            name: id.to_owned(),
            device_id: "dev".to_owned(),
            zone_name: None,
            position: NormalizedPosition { x: 0.5, y: 0.5 },
            size: NormalizedPosition { x: w, y: h },
            rotation: 0.0,
            scale: 1.0,
            display_order: order,
            orientation: None,
            topology: LedTopology::Strip {
                count: 1,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            attachment: None,
            brightness: None,
        }
    }

    #[test]
    fn larger_boxes_rank_lower() {
        let ranks = rank_by_area(&[
            zone("strip", 0.3, 0.02, 0),
            zone("fan", 0.3, 0.3, 1),
            zone("pump", 0.1, 0.1, 2),
        ]);
        assert_eq!(ranks["fan"], 0);
        assert_eq!(ranks["pump"], 1);
        assert_eq!(ranks["strip"], 2);
    }

    #[test]
    fn equal_areas_keep_display_order() {
        let ranks = rank_by_area(&[zone("b", 0.1, 0.1, 5), zone("a", 0.1, 0.1, 1)]);
        assert_eq!(ranks["a"], 0);
        assert_eq!(ranks["b"], 1);
    }

    #[test]
    fn tiers_outrank_area() {
        let big_primary = z_index(
            Tier {
                primary: true,
                ..Tier::default()
            },
            0,
        );
        let small_pointer = z_index(
            Tier {
                under_pointer: true,
                ..Tier::default()
            },
            50,
        );
        let small_lifted = z_index(
            Tier {
                lifted: true,
                ..Tier::default()
            },
            50,
        );
        let small_base = z_index(Tier::default(), 50);
        assert!(big_primary > small_pointer);
        assert!(small_pointer > small_lifted);
        assert!(small_lifted > small_base);
    }
}
