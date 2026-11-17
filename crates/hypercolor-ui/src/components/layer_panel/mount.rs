//! Which mounted row a layer belongs to.
//!
//! A whole-layer replace mints a new layer id, so a row keyed by its
//! layer's id is torn down and rebuilt each time a blend, transform, or
//! color value is written. A row is keyed by the id it first mounted under
//! instead, and a layer that takes a vanished layer's place in the list
//! inherits that layer's row.

/// A row to mount: the zone whose stack holds its layer, and what keeps
/// it mounted.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RowKey {
    pub scene_id: String,
    pub zone_id: String,
    /// The id the row first mounted under, and its layer's source.
    pub mount: (String, String),
    /// One of a screen's own layers.
    pub on_screen: bool,
}

/// A layer as the list names it now, top of the list first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowSlot {
    pub scene_id: String,
    pub zone_id: String,
    pub layer_id: String,
    /// The layer's source identity. `None` while the stack that holds the
    /// layer and the list that names it disagree, which lasts until
    /// whichever of the two is behind catches up.
    pub source: Option<String>,
    pub on_screen: bool,
}

/// A mounted row and the layer it shows now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountedRow {
    pub key: RowKey,
    pub layer_id: String,
}

/// Carry each row's mount across a change of the list.
///
/// A layer the list already showed keeps its row. A layer the list has
/// not seen takes the row at its place when that row's layer is gone and
/// had the same source: that is what a replace leaves. A layer whose
/// source cannot be read yet holds the row at its place rather than
/// dropping it, and one with neither a source nor a row is left out.
#[must_use]
pub fn inherit_mounts(previous: &[MountedRow], next: &[RowSlot]) -> Vec<MountedRow> {
    next.iter()
        .enumerate()
        .filter_map(|(place, slot)| {
            let beside = |row: &&MountedRow| {
                row.key.scene_id == slot.scene_id
                    && row.key.zone_id == slot.zone_id
                    && row.key.on_screen == slot.on_screen
            };
            let listed = |row: &MountedRow| {
                next.iter()
                    .any(|other| other.zone_id == row.key.zone_id && other.layer_id == row.layer_id)
            };
            let held = previous
                .iter()
                .filter(beside)
                .find(|row| row.layer_id == slot.layer_id)
                .or_else(|| {
                    previous.get(place).filter(beside).filter(|row| {
                        !listed(row)
                            && slot
                                .source
                                .as_ref()
                                .is_none_or(|source| *source == row.key.mount.1)
                    })
                });
            let mount = match (held, &slot.source) {
                (Some(row), source) => (
                    row.key.mount.0.clone(),
                    source.clone().unwrap_or_else(|| row.key.mount.1.clone()),
                ),
                (None, Some(source)) => (slot.layer_id.clone(), source.clone()),
                (None, None) => return None,
            };
            Some(MountedRow {
                key: RowKey {
                    scene_id: slot.scene_id.clone(),
                    zone_id: slot.zone_id.clone(),
                    mount,
                    on_screen: slot.on_screen,
                },
                layer_id: slot.layer_id.clone(),
            })
        })
        .collect()
}
