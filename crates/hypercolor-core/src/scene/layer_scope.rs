//! How a layer's output scope behaves as stacks and memberships change.
//!
//! A scoped layer names outputs of its own zone. These rules keep that
//! true without ever widening a scope: a request may only introduce ids
//! the zone holds, a layer follows its outputs when they move to another
//! zone together, and the stack-wide gestures (apply, clear) leave the
//! layers they are not about alone.

use std::collections::{HashMap, HashSet};

use hypercolor_types::api::scene::{LayerMove, LayerSlot};
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{LayerScope, LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{Scene, Zone, ZoneId};
use hypercolor_types::spatial::Output;

use super::LayerMutationError;
use crate::spatial::is_display_zone;

/// One output a membership edit moves from one zone of a scene to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputMove {
    /// The output's id in both zones' layouts.
    pub output_id: String,
    /// The zone that holds the output before the edit.
    pub from: ZoneId,
    /// The zone that holds it afterwards.
    pub to: ZoneId,
}

/// Refuse a scope that introduces an id the zone does not hold.
///
/// `carried` is the scope of the layer being replaced, whose ids may be
/// dormant and are not re-checked. A new layer carries nothing.
pub(super) fn check_introduced_scope(
    zone: &Zone,
    layer: &SceneLayer,
    carried: Option<&LayerScope>,
) -> Result<(), LayerMutationError> {
    let Some(scope) = &layer.scope else {
        return Ok(());
    };
    let missing = scope.outputs.iter().find(|id| {
        !carried.is_some_and(|carried| carried.contains(id))
            && !zone.layout.zones.iter().any(|output| output.id == **id)
    });
    match missing {
        Some(output) => Err(LayerMutationError::ScopeOutputMissing {
            layer_id: layer.id,
            output: output.clone(),
        }),
        None => Ok(()),
    }
}

/// Replace a zone's whole-zone layers with `base`, keeping every scoped
/// layer above it in its existing order, dormant ones included.
pub(super) fn replace_unscoped_layers(zone: &mut Zone, base: SceneLayer) {
    let mut layers = Vec::with_capacity(zone.layers.len() + 1);
    layers.push(base);
    layers.extend(zone.layers.drain(..).filter(|layer| layer.scope.is_some()));
    zone.layers = layers;
}

/// Remove a zone's lighting: every layer goes except those
/// [`is_screen_only`] proves are faces. Returns whether any layer was
/// removed.
pub(super) fn clear_lighting_layers(zone: &mut Zone) -> bool {
    let before = zone.layers.len();
    let Zone { layers, layout, .. } = zone;
    layers.retain(|layer| is_screen_only(&layout.zones, layer));
    zone.layers.len() != before
}

/// Whether every id in a layer's scope is a live output of the zone and a
/// screen. Anything else is lighting.
///
/// The zone's own layout says which outputs are screens, so an id that
/// names no current output cannot be shown to be one. A layer scoped to a
/// screen and a strip is lighting whether or not the strip is still in the
/// zone, or re-adding the strip would light it after a stop. The cost is
/// accepted: a face whose screen has left the zone is removed by a clear.
fn is_screen_only(outputs: &[Output], layer: &SceneLayer) -> bool {
    let Some(scope) = &layer.scope else {
        return false;
    };
    !scope.outputs.is_empty()
        && scope.outputs.iter().all(|id| {
            outputs
                .iter()
                .any(|output| output.id == *id && is_display_zone(output))
        })
}

/// The effect a stop gesture ends in this zone: the first effect layer
/// that clearing the zone removes. A face that survives the clear is not
/// stopped, so it is never the answer.
#[must_use]
pub fn first_lighting_effect(zone: &Zone) -> Option<EffectId> {
    zone.layers
        .iter()
        .filter(|layer| !is_screen_only(&zone.layout.zones, layer))
        .find_map(|layer| match layer.source {
            LayerSource::Effect { effect_id, .. } => Some(effect_id),
            _ => None,
        })
}

/// The first whole-zone effect layer: what applying an effect replaces.
pub(super) fn base_effect_layer(zone: &Zone) -> Option<&SceneLayer> {
    zone.layers
        .iter()
        .find(|layer| layer.scope.is_none() && matches!(layer.source, LayerSource::Effect { .. }))
}

/// What happens to one scoped layer when outputs leave its zone.
enum Carry {
    /// Every live output went to one zone, and the layer goes with them.
    Move(ZoneId),
    /// Some live outputs stayed; the ones that left drop out of the scope.
    Narrow(LayerScope),
}

/// Decide which scoped layers follow a set of outputs between zones.
///
/// Call this before the membership edit is applied: which of a scope's
/// ids are live is read from the zones as they stand. Each layer of a
/// zone that loses outputs is handled against everything the edit moves
/// out of that zone:
///
/// - No scope, or no scope id among the moved outputs: unchanged.
/// - Every live scope id moves to the same zone: the layer moves to the
///   top of that zone's stack, keeping its id, its order among the layers
///   moved with it, and its scope.
/// - Some live ids move and some stay: the layer stays and the moved ids
///   leave its scope.
/// - Every live id moves but to different zones: the layer stays with its
///   scope intact and goes dormant, because no one zone holds what it
///   named and dropping every id would leave an empty scope.
///
/// A layer whose id the destination already uses also stays, and so does
/// the second of two layers that share an id and would land in the same
/// zone. Layer ids are unique within a zone, not across a scene, and a
/// carry never overwrites a layer or drops one.
#[must_use]
pub fn plan_layer_carry(scene: &Scene, moves: &[OutputMove]) -> Vec<LayerMove> {
    let mut carries: Vec<(ZoneId, SceneLayerId, Carry)> = Vec::new();
    for zone in &scene.zones {
        let leaving: HashMap<&str, ZoneId> = moves
            .iter()
            .filter(|moved| moved.from == zone.id && moved.to != zone.id)
            .filter(|moved| scene.zones.iter().any(|target| target.id == moved.to))
            .filter(|moved| {
                zone.layout
                    .zones
                    .iter()
                    .any(|output| output.id == moved.output_id)
            })
            .map(|moved| (moved.output_id.as_str(), moved.to))
            .collect();
        if leaving.is_empty() {
            continue;
        }
        for layer in &zone.layers {
            let Some(scope) = &layer.scope else {
                continue;
            };
            let Some(carry) = carry_for(scene, zone, layer.id, scope, &leaving) else {
                continue;
            };
            let lands_on_a_twin = carries.iter().any(|(_, carried, prior)| {
                *carried == layer.id
                    && matches!(
                        (prior, &carry),
                        (Carry::Move(taken), Carry::Move(wanted)) if taken == wanted
                    )
            });
            if !lands_on_a_twin {
                carries.push((zone.id, layer.id, carry));
            }
        }
    }
    if carries.is_empty() {
        return Vec::new();
    }

    // Lay out every stack as the edit leaves it, so each record can name
    // the index its layer ends at.
    let moved_out: HashSet<(ZoneId, SceneLayerId)> = carries
        .iter()
        .filter(|(_, _, carry)| matches!(carry, Carry::Move(_)))
        .map(|(zone_id, layer_id, _)| (*zone_id, *layer_id))
        .collect();
    let mut final_stacks: HashMap<ZoneId, Vec<SceneLayerId>> = scene
        .zones
        .iter()
        .map(|zone| {
            let stack = zone
                .layers
                .iter()
                .map(|layer| layer.id)
                .filter(|layer_id| !moved_out.contains(&(zone.id, *layer_id)))
                .collect();
            (zone.id, stack)
        })
        .collect();
    for (_, layer_id, carry) in &carries {
        if let Carry::Move(destination) = carry
            && let Some(stack) = final_stacks.get_mut(destination)
        {
            stack.push(*layer_id);
        }
    }

    carries
        .into_iter()
        .filter_map(|(source, layer_id, carry)| {
            let zone = scene.zones.iter().find(|zone| zone.id == source)?;
            let from_index = zone.layers.iter().position(|layer| layer.id == layer_id)?;
            let scope_before = zone.layers[from_index].scope.clone()?;
            let (destination, scope_after) = match carry {
                Carry::Move(destination) => (destination, scope_before.clone()),
                Carry::Narrow(scope_after) => (source, scope_after),
            };
            let to_index = final_stacks
                .get(&destination)?
                .iter()
                .position(|id| *id == layer_id)?;
            Some(LayerMove {
                layer_id,
                from: LayerSlot {
                    zone_id: source,
                    index: from_index,
                },
                to: LayerSlot {
                    zone_id: destination,
                    index: to_index,
                },
                scope_before,
                scope_after,
            })
        })
        .collect()
}

fn carry_for(
    scene: &Scene,
    zone: &Zone,
    layer_id: SceneLayerId,
    scope: &LayerScope,
    leaving: &HashMap<&str, ZoneId>,
) -> Option<Carry> {
    if !scope
        .outputs
        .iter()
        .any(|id| leaving.contains_key(id.as_str()))
    {
        return None;
    }
    let live = zone
        .scoped_outputs(scope)
        .map(|output| output.id.as_str())
        .collect::<Vec<_>>();
    let destinations = live
        .iter()
        .map(|id| leaving.get(id).copied())
        .collect::<Option<HashSet<ZoneId>>>();
    match destinations {
        // Every live id leaves.
        Some(destinations) => {
            let mut destinations = destinations.into_iter();
            let destination = destinations.next()?;
            if destinations.next().is_some() {
                return None;
            }
            let taken = scene
                .zones
                .iter()
                .find(|target| target.id == destination)
                .is_some_and(|target| target.layers.iter().any(|layer| layer.id == layer_id));
            (!taken).then_some(Carry::Move(destination))
        }
        // At least one live id stays.
        None => Some(Carry::Narrow(LayerScope {
            outputs: scope
                .outputs
                .iter()
                .filter(|id| !leaving.contains_key(id.as_str()))
                .cloned()
                .collect(),
        })),
    }
}

/// Check a caller-supplied set of layer moves against the scene as it
/// stands, with the membership half of the edit already applied.
///
/// Each move must find its layer at the stated slot with the stated
/// scope, must not introduce a scope id its destination does not hold,
/// and must leave every stack addressable at the indices it names.
pub(super) fn validate_layer_moves(
    scene: &Scene,
    moves: &[LayerMove],
) -> Result<(), LayerMutationError> {
    let zone = |zone_id: ZoneId| {
        scene
            .zones
            .iter()
            .find(|zone| zone.id == zone_id)
            .ok_or(LayerMutationError::ZoneMissing)
    };
    // A layer id is unique within a zone, not across a scene, so a move is
    // identified by its source zone and id, and two moves may not land the
    // same id in one zone.
    let mut moving = HashSet::new();
    let mut landing = HashSet::new();
    for moved in moves {
        let layer_id = moved.layer_id;
        if !moving.insert((moved.from.zone_id, layer_id))
            || !landing.insert((moved.to.zone_id, layer_id))
        {
            return Err(LayerMutationError::DuplicateLayer { layer_id });
        }
        let source = zone(moved.from.zone_id)?;
        let destination = zone(moved.to.zone_id)?;
        let layer = source
            .layers
            .get(moved.from.index)
            .filter(|layer| layer.id == layer_id)
            .filter(|layer| layer.scope.as_ref() == Some(&moved.scope_before))
            .ok_or(LayerMutationError::LayerMoveStale { layer_id })?;

        if source.id != destination.id
            && destination
                .layers
                .iter()
                .any(|resident| resident.id == layer_id)
        {
            return Err(LayerMutationError::DuplicateLayer { layer_id });
        }

        let mut carried = layer.clone();
        carried.scope = Some(moved.scope_after.clone());
        carried
            .validate()
            .map_err(|errors| LayerMutationError::InvalidLayer { errors })?;
        check_introduced_scope(destination, &carried, Some(&moved.scope_before))?;
    }

    for target in &scene.zones {
        let arriving = moves
            .iter()
            .filter(|moved| moved.to.zone_id == target.id)
            .collect::<Vec<_>>();
        if arriving.is_empty() {
            continue;
        }
        let leaving = moves
            .iter()
            .filter(|moved| moved.from.zone_id == target.id)
            .count();
        let staying = target.layers.len() - leaving;
        let len = staying + arriving.len();
        let mut indices = HashSet::new();
        for moved in arriving {
            if moved.to.index >= len || !indices.insert(moved.to.index) {
                return Err(LayerMutationError::InvalidIndex {
                    index: moved.to.index,
                    len,
                });
            }
        }
    }
    Ok(())
}

/// Carry out layer moves that are known to fit the scene: a plan from
/// [`plan_layer_carry`], or a set [`validate_layer_moves`] accepted.
///
/// Every moved layer leaves its stack first, then each lands at its final
/// index in ascending order, so an index always names a position in the
/// finished stack. Returns the zones whose stacks changed.
pub(super) fn execute_layer_moves(scene: &mut Scene, moves: &[LayerMove]) -> Vec<ZoneId> {
    let mut lifted: HashMap<(ZoneId, SceneLayerId), SceneLayer> = HashMap::new();
    let mut touched = Vec::new();
    for moved in moves {
        let Some(zone) = scene
            .zones
            .iter_mut()
            .find(|zone| zone.id == moved.from.zone_id)
        else {
            continue;
        };
        if let Some(index) = zone
            .layers
            .iter()
            .position(|layer| layer.id == moved.layer_id)
        {
            lifted.insert((zone.id, moved.layer_id), zone.layers.remove(index));
            if !touched.contains(&zone.id) {
                touched.push(zone.id);
            }
        }
    }

    let mut landing = moves.iter().collect::<Vec<_>>();
    landing.sort_by_key(|moved| moved.to.index);
    for moved in landing {
        let Some(mut layer) = lifted.remove(&(moved.from.zone_id, moved.layer_id)) else {
            continue;
        };
        let Some(zone) = scene
            .zones
            .iter_mut()
            .find(|zone| zone.id == moved.to.zone_id)
        else {
            continue;
        };
        layer.scope = Some(moved.scope_after.normalized());
        let index = moved.to.index.min(zone.layers.len());
        zone.layers.insert(index, layer);
        if !touched.contains(&zone.id) {
            touched.push(zone.id);
        }
    }

    for zone in &mut scene.zones {
        if touched.contains(&zone.id) {
            zone.layers_version = zone.layers_version.saturating_add(1);
        }
    }
    touched
}
