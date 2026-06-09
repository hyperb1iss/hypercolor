use std::f32::consts::PI;

use hypercolor_types::spatial::{NormalizedPosition, SpatialLayout};

use super::{GRID_EPSILON, clamp_zone_center, normalize_rotation};

// ── Compound geometry ────────────────────────────────────────────────────

/// Axis-aligned bounding box enclosing a set of zones.
#[derive(Debug, Clone)]
pub struct CompoundBounds {
    pub center: NormalizedPosition,
    pub size: NormalizedPosition,
}

/// Compute the axis-aligned bounding box of all zones in `zone_ids`.
pub fn compound_bounding_box(
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
pub fn translate_zones(
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
pub fn group_centroid(
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
pub fn translate_group(
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
pub fn rotate_group(
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
pub fn scale_group(
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
pub enum AlignAxis {
    X,
    Y,
}

/// Which edge or center to align selected zones against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignAnchor {
    /// Left edge (X axis) or top edge (Y axis).
    Min,
    /// Bbox center on the chosen axis.
    Center,
    /// Right edge (X axis) or bottom edge (Y axis).
    Max,
}

/// Align each zone in `zone_ids` to a common edge or center of the group's
/// bounding box on `axis`. Zones on the other axis are left untouched.
pub fn align_group(
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
pub fn distribute_group(
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
pub fn pack_group(
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
pub fn mirror_group(
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
