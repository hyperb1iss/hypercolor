//! A zone's layer stack in the journal: layers added, removed, and put
//! in another order, each as one step.
//!
//! One face on the screens of several zones is one layer per zone, written
//! in sequence because each write is fenced by the scene revision the one
//! before it left. They are still one thing somebody did, so they are one
//! journal entry: Undo removes every layer it made and Redo makes them
//! again, each as it stood when it was removed.
//!
//! A removed layer is kept whole, with the place it held in its stack, so
//! Undo makes it again there. The daemon makes every layer on top of its
//! stack and under a new id, so the undo is a create and then a reorder,
//! and every step that names the layer follows it to the new id.
//!
//! A reorder keeps the stack's order before and after. It replays only
//! onto a stack that still holds exactly those layers; onto any other
//! there is no order of it to go back to, and the step is dropped.

use hypercolor_types::api::scene::CreateLayerRequest;
use hypercolor_types::layer::{LayerScope, SceneLayer, SceneLayerId};
use leptos::prelude::*;

use super::{EntryId, Fence, Settled, StudioEdit, StudioHistory, Waiting, refuses_step};
use crate::components::layer_panel::scope::reorder_within;
use crate::{api, toasts};

/// One layer to create: where, what, and the names of what it lands on,
/// for the report.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerAdd {
    pub zone_id: String,
    pub request: CreateLayerRequest,
    pub lands_on: Vec<String>,
}

/// A layer an add made, as the journal keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct AddedLayer {
    /// The zone it was created in. A move may have carried it on since.
    pub zone_id: String,
    /// The layer as created, or as it stood when an undo removed it.
    pub layer: SceneLayer,
    pub lands_on: Vec<String>,
}

/// A layer a removal took out of its zone's stack, as the journal keeps
/// it.
#[derive(Clone, Debug, PartialEq)]
pub struct RemovedLayer {
    /// The zone whose stack held it.
    pub zone_id: String,
    /// What it went by, for the announcement.
    pub name: String,
    /// Where it sat in the stack, bottom first.
    pub index: usize,
    /// The layer as it stood when it was removed, or as an undo made it
    /// again.
    pub layer: SceneLayer,
}

/// How an add came out, for whoever asked: the names it landed on and the
/// names it did not, and the outputs the new layers are limited to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddOutcome {
    pub landed: Vec<String>,
    pub failed: Vec<String>,
    pub outputs: Vec<String>,
}

/// Hears how an add came out, once. A boxed closure, not a `Callback`: an
/// add is asked for from the picker's click, and a `Callback` made there
/// is disposed with the picker, before the write lands.
pub type AddDone = Box<dyn FnOnce(AddOutcome)>;

/// The request that makes `layer` again, limited to the outputs of its
/// scope `zone` still holds. `None` when it named outputs and none is
/// left: the layer would have nothing to draw on.
#[must_use]
pub fn recreate_request(
    layer: &SceneLayer,
    zone: &api::ZoneResource,
) -> Option<CreateLayerRequest> {
    let scope = match &layer.scope {
        None => None,
        Some(scope) => {
            let outputs: Vec<String> = scope
                .outputs
                .iter()
                .filter(|id| zone.members.iter().any(|member| member.id.0 == **id))
                .cloned()
                .collect();
            if outputs.is_empty() {
                return None;
            }
            Some(LayerScope { outputs })
        }
    };
    Some(CreateLayerRequest {
        source: layer.source.clone(),
        name: layer.name.clone(),
        blend: Some(layer.blend),
        opacity: Some(layer.opacity),
        transform: Some(layer.transform),
        adjust: Some(layer.adjust),
        bindings: Some(layer.bindings.clone()),
        enabled: Some(layer.enabled),
        scope,
    })
}

/// The order that puts `restored`, which a create just left on top of
/// `stack`, back at `index`. Both count from the bottom, and a stack now
/// too short for `index` keeps the layer on top. `None` when the layer
/// already sits there, or the stack does not hold it.
#[must_use]
pub fn restored_order(
    stack: &[SceneLayerId],
    restored: SceneLayerId,
    index: usize,
) -> Option<Vec<SceneLayerId>> {
    let from = stack.iter().position(|id| *id == restored)?;
    let to = index.min(stack.len().saturating_sub(1));
    if from == to {
        return None;
    }
    let mut order = stack.to_vec();
    let layer = order.remove(from);
    order.insert(to, layer);
    Some(order)
}

/// The order an undo or redo of a reorder sends: `wanted`, while `live`
/// holds exactly the layers it names. `None` once a layer has joined or
/// left the stack since, when no order of the recorded layers describes
/// the stack any longer.
#[must_use]
pub fn replayable_order(
    live: &[SceneLayerId],
    wanted: &[SceneLayerId],
) -> Option<Vec<SceneLayerId>> {
    let same = live.len() == wanted.len()
        && live.iter().all(|id| wanted.contains(id))
        && wanted.iter().all(|id| live.contains(id));
    same.then(|| wanted.to_vec())
}

/// The order `stack` takes when `moving` trades places with its neighbor
/// among the layers `group` names: `delta` of 1 moves it up, -1 down. The
/// group is read in the stack's own order, so a reorder asked for against
/// an older view of the stack still moves the layer one place from where
/// it sits now. `None` when there is no such neighbor.
#[must_use]
pub fn reorder_step(
    stack: &[SceneLayerId],
    group: &[SceneLayerId],
    moving: SceneLayerId,
    delta: isize,
) -> Option<Vec<SceneLayerId>> {
    let group: Vec<SceneLayerId> = stack
        .iter()
        .filter(|id| group.contains(id))
        .copied()
        .collect();
    reorder_within(stack, &group, moving, delta)
}

/// The ids of a stack, bottom first.
fn order_of(stack: &[SceneLayer]) -> Vec<SceneLayerId> {
    stack.iter().map(|layer| layer.id).collect()
}

/// Where the layer `layer_id` sits in `scene`: the zone it was created
/// in, or wherever a move has carried it since.
fn locate(
    scene: &api::SceneDocument,
    zone_id: &str,
    layer_id: SceneLayerId,
) -> Option<(api::ZoneResource, SceneLayer)> {
    let holds = |zone: &&api::ZoneResource| zone.layers.iter().any(|layer| layer.id == layer_id);
    let zone = scene
        .zones
        .iter()
        .find(|zone| zone.id.to_string() == zone_id && holds(zone))
        .or_else(|| scene.zones.iter().find(holds))?;
    let layer = zone.layers.iter().find(|layer| layer.id == layer_id)?;
    Some((zone.clone(), layer.clone()))
}

impl StudioHistory {
    /// Create `adds` in order as one undo step called `name`, and tell
    /// `done` how it came out. A failure stops the rest, which are
    /// reported with it; what landed before stays, and is what the step
    /// undoes.
    pub fn add_layers(self, name: String, adds: Vec<LayerAdd>, done: AddDone) {
        self.when_free(Waiting::AddLayers { name, adds, done });
    }

    pub(super) fn add_layers_now(self, name: String, adds: Vec<LayerAdd>, done: AddDone) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(generation) = self.begin() else {
            return;
        };
        leptos::task::spawn_local(async move {
            let mut fence = Fence::of(&scene);
            let mut made: Vec<AddedLayer> = Vec::new();
            let mut outcome = AddOutcome::default();
            for add in adds {
                if !outcome.failed.is_empty() {
                    outcome.failed.extend(add.lands_on);
                    continue;
                }
                let result = api::create_layer(&add.zone_id, &add.request, Some(&fence)).await;
                if !self.is_current(generation) {
                    return;
                }
                match result {
                    Ok(api::LayerStackOutcome::Applied(stack)) => {
                        fence = fence.at(stack.revision);
                        if let Some(layer) = stack.items.last().cloned() {
                            outcome.outputs.extend(
                                layer
                                    .scope
                                    .iter()
                                    .flat_map(|scope| scope.outputs.iter().cloned()),
                            );
                            made.push(AddedLayer {
                                zone_id: add.zone_id.clone(),
                                layer,
                                lands_on: add.lands_on.clone(),
                            });
                        }
                        outcome.landed.extend(add.lands_on);
                        self.adopt_written(&add.zone_id, stack);
                    }
                    Ok(api::LayerStackOutcome::Stale { .. }) | Err(_) => {
                        outcome.failed.extend(add.lands_on);
                    }
                }
            }
            if !made.is_empty() {
                self.journal.update(|journal| {
                    journal.record(StudioEdit::LayersAdded { name, layers: made });
                });
            }
            if !outcome.failed.is_empty() {
                self.resync(generation).await;
            }
            done(outcome);
            self.refresh.run(());
            self.finish(generation);
        });
    }

    /// Take in the stack a layer write of ours left in the zone `zone_id`.
    fn adopt_written(self, zone_id: &str, stack: api::LayerStackResponse) {
        let Some(base) = self.current_scene() else {
            return;
        };
        if let Some(zone) = base
            .zones
            .iter()
            .find(|zone| zone.id.to_string() == zone_id)
        {
            self.adopt_stack(&base, zone, stack);
        }
    }

    /// Undo or redo the add at the journal place `id`.
    pub(super) fn replay_layers_added(
        self,
        id: EntryId,
        redo: bool,
        name: String,
        layers: Vec<AddedLayer>,
    ) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(generation) = self.begin() else {
            return;
        };
        leptos::task::spawn_local(async move {
            let mut fence = Fence::of(&scene);
            // What the step holds once this replay is through: the layers
            // an undo removed, as they stood, or the ones a redo made.
            let mut replayed: Vec<AddedLayer> = Vec::new();
            // The layers the replay did not get to.
            let mut left: Vec<AddedLayer> = Vec::new();
            let mut trouble: Option<String> = None;
            let order: Vec<AddedLayer> = if redo {
                layers
            } else {
                layers.into_iter().rev().collect()
            };
            for added in order {
                if trouble.is_some() {
                    left.push(added);
                    continue;
                }
                let Some(current) = self.current_scene() else {
                    return;
                };
                let result = if redo {
                    let request = current
                        .zones
                        .iter()
                        .find(|zone| zone.id.to_string() == added.zone_id)
                        .and_then(|zone| recreate_request(&added.layer, zone));
                    // Its zone, or every output it named, is gone: there is
                    // nothing to put it back on.
                    let Some(request) = request else {
                        continue;
                    };
                    api::create_layer(&added.zone_id, &request, Some(&fence))
                        .await
                        .map(|outcome| (outcome, added.zone_id.clone(), None))
                } else {
                    // Already removed some other way: nothing to undo.
                    let Some((zone, layer)) = locate(&current, &added.zone_id, added.layer.id)
                    else {
                        continue;
                    };
                    let zone_id = zone.id.to_string();
                    api::delete_layer(&zone_id, &layer.id.to_string(), Some(&fence))
                        .await
                        .map(|outcome| (outcome, zone_id, Some(layer)))
                };
                if !self.is_current(generation) {
                    return;
                }
                match result {
                    Ok((api::LayerStackOutcome::Applied(stack), zone_id, removed)) => {
                        fence = fence.at(stack.revision);
                        let layer = match removed {
                            Some(layer) => Some(layer),
                            None => stack.items.last().cloned(),
                        };
                        // A redo makes the layer again under a new id, and
                        // every later step that names it has to follow, or
                        // its own redo finds no such layer.
                        if redo
                            && let Some(made) = &layer
                            && made.id != added.layer.id
                        {
                            self.journal.try_update(|journal| {
                                journal.remap_layer(&zone_id, added.layer.id, made.id);
                            });
                        }
                        if let Some(layer) = layer {
                            replayed.push(AddedLayer {
                                zone_id: zone_id.clone(),
                                layer,
                                lands_on: added.lands_on.clone(),
                            });
                        }
                        self.adopt_written(&zone_id, stack);
                    }
                    Ok((api::LayerStackOutcome::Stale { .. }, ..)) => {
                        trouble = Some(
                            "Scene changed elsewhere; history was not applied. Try again."
                                .to_owned(),
                        );
                        left.push(added);
                    }
                    // A layer the daemon will not take back, or cannot
                    // find, is done with; the rest go on.
                    Err(error) if refuses_step(&error) => {}
                    Err(error) => {
                        trouble = Some(format!("History could not be applied: {error}"));
                        left.push(added);
                    }
                }
            }
            if let Some(trouble) = &trouble {
                toasts::toast_error(trouble);
                self.resync(generation).await;
            }
            // An undo walked the layers top down; the step keeps them in
            // the order they were made.
            if !redo {
                replayed.reverse();
                left.reverse();
            }
            let entry = |layers| StudioEdit::LayersAdded {
                name: name.clone(),
                layers,
            };
            let settled = match (redo, replayed.is_empty(), left.is_empty()) {
                // Nothing of the step is left either way.
                (_, true, true) => Some(Settled::Gone),
                // Stopped before anything changed: the step stands.
                (_, true, false) => None,
                (false, false, true) => Some(Settled::Unapplied(entry(replayed))),
                (true, false, true) => Some(Settled::Applied(entry(replayed))),
                // Stopped partway. The step keeps what it can still act
                // on: the layers an undo has not removed yet, or the ones
                // a redo has made.
                (false, false, false) => Some(Settled::Applied(entry(left))),
                (true, false, false) => Some(Settled::Applied(entry(replayed))),
            };
            let finished = trouble.is_none();
            if let Some(settled) = settled {
                let gone = settled == Settled::Gone;
                self.journal.update(|journal| journal.settle(id, settled));
                if gone {
                    self.replay_words.try_set_value(None);
                    toasts::toast_info(&super::dropped_step_notice(
                        redo,
                        &format!("{name} and what it was on are gone"),
                    ));
                } else if finished {
                    self.announce_replay();
                }
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }

    /// Take the layer `layer_id` out of the zone `zone_id` as one undo
    /// step. `name` is what the layer goes by, for the announcement.
    pub fn remove_layer(self, zone_id: String, layer_id: SceneLayerId, name: String) {
        self.when_free(Waiting::RemoveLayer {
            zone_id,
            layer_id,
            name,
        });
    }

    pub(super) fn remove_layer_now(self, zone_id: String, layer_id: SceneLayerId, name: String) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let found = super::find_layer(&scene, &zone_id, layer_id).and_then(|(zone, layer)| {
            let index = zone.layers.iter().position(|held| held.id == layer_id)?;
            Some((index, layer.clone()))
        });
        // Already gone, as after a second press on its button: what was
        // asked for holds, and there is nothing to take back.
        let Some((index, layer)) = found else {
            return;
        };
        let Some(generation) = self.begin() else {
            return;
        };
        leptos::task::spawn_local(async move {
            let fence = Fence::of(&scene);
            let result = api::delete_layer(&zone_id, &layer_id.to_string(), Some(&fence)).await;
            if !self.is_current(generation) {
                return;
            }
            match result {
                Ok(api::LayerStackOutcome::Applied(stack)) => {
                    self.journal.update(|journal| {
                        journal.record(StudioEdit::LayerRemoved(Box::new(RemovedLayer {
                            zone_id: zone_id.clone(),
                            name,
                            index,
                            layer,
                        })));
                    });
                    self.adopt_written(&zone_id, stack);
                    toasts::toast_success("Layer removed");
                }
                Ok(api::LayerStackOutcome::Stale { .. }) => {
                    toasts::toast_error("Layer stack changed elsewhere, so it was reloaded");
                    self.resync(generation).await;
                }
                Err(error) => toasts::toast_error(&format!("Layer delete failed: {error}")),
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }

    /// Trade the layer `layer_id` of the zone `zone_id` with its neighbor
    /// among the layers `group` names, as one undo step: `delta` of 1
    /// moves it up, -1 down. `name` is what the layer goes by.
    pub fn reorder_layer(
        self,
        zone_id: String,
        group: Vec<SceneLayerId>,
        layer_id: SceneLayerId,
        delta: isize,
        name: String,
    ) {
        self.when_free(Waiting::ReorderLayer {
            zone_id,
            group,
            layer_id,
            delta,
            name,
        });
    }

    pub(super) fn reorder_layer_now(
        self,
        zone_id: String,
        group: &[SceneLayerId],
        layer_id: SceneLayerId,
        delta: isize,
        name: String,
    ) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(before) = scene
            .zones
            .iter()
            .find(|zone| zone.id.to_string() == zone_id)
            .map(|zone| order_of(&zone.layers))
        else {
            return;
        };
        // Already at the edge of its group: there is nowhere to move it.
        let Some(order) = reorder_step(&before, group, layer_id, delta) else {
            return;
        };
        let Some(generation) = self.begin() else {
            return;
        };
        leptos::task::spawn_local(async move {
            let fence = Fence::of(&scene);
            let result = api::reorder_layers(&zone_id, order, Some(&fence)).await;
            if !self.is_current(generation) {
                return;
            }
            match result {
                Ok(api::LayerStackOutcome::Applied(stack)) => {
                    let after = order_of(&stack.items);
                    self.journal.update(|journal| {
                        journal.record(StudioEdit::LayersReordered {
                            zone_id: zone_id.clone(),
                            name,
                            before,
                            after,
                        });
                    });
                    self.adopt_written(&zone_id, stack);
                }
                Ok(api::LayerStackOutcome::Stale { .. }) => {
                    toasts::toast_error("Layer stack changed elsewhere, so it was reloaded");
                    self.resync(generation).await;
                }
                Err(error) => toasts::toast_error(&format!("Layer reorder failed: {error}")),
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }

    /// The stack a replayed layer write left, or `None` once what stopped
    /// it is dealt with: a stale fence reads the scene again and keeps the
    /// step, a refusal drops the step (see [`refuses_step`]), and anything
    /// else keeps it for another try.
    async fn replayed_stack(
        self,
        generation: u64,
        id: EntryId,
        redo: bool,
        result: api::ApiResult<api::LayerStackOutcome>,
    ) -> Option<api::LayerStackResponse> {
        match result {
            Ok(api::LayerStackOutcome::Applied(stack)) => Some(stack),
            Ok(api::LayerStackOutcome::Stale { .. }) => {
                toasts::toast_error("Scene changed elsewhere; history was not applied. Try again.");
                self.resync(generation).await;
                None
            }
            Err(error) if refuses_step(&error) => {
                self.drop_step(id, redo, &error.to_string());
                None
            }
            Err(error) => {
                toasts::toast_error(&format!("History could not be applied: {error}"));
                None
            }
        }
    }

    /// Undo or redo the removal at the journal place `id`: an undo makes
    /// the layer again where it sat, a redo takes it out again.
    pub(super) fn replay_layer_removed(self, id: EntryId, redo: bool, removed: RemovedLayer) {
        if redo {
            self.remove_again(id, removed);
        } else {
            self.restore_removed(id, removed);
        }
    }

    fn restore_removed(self, id: EntryId, removed: RemovedLayer) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let RemovedLayer {
            zone_id,
            name,
            index,
            layer,
        } = removed;
        let Some(zone) = scene
            .zones
            .iter()
            .find(|zone| zone.id.to_string() == zone_id)
        else {
            self.drop_step(
                id,
                false,
                &format!("the zone {name} was in is no longer in this scene"),
            );
            return;
        };
        let Some(request) = recreate_request(&layer, zone) else {
            self.drop_step(
                id,
                false,
                &format!("the outputs {name} was on have left this zone"),
            );
            return;
        };
        let Some(generation) = self.begin() else {
            return;
        };
        leptos::task::spawn_local(async move {
            let fence = Fence::of(&scene);
            let result = api::create_layer(&zone_id, &request, Some(&fence)).await;
            if !self.is_current(generation) {
                return;
            }
            if let Some(mut stack) = self.replayed_stack(generation, id, false, result).await {
                let made = stack.items.last().cloned().unwrap_or_else(|| layer.clone());
                if let Some(order) = restored_order(&order_of(&stack.items), made.id, index) {
                    let placed =
                        api::reorder_layers(&zone_id, order, Some(&fence.at(stack.revision))).await;
                    if !self.is_current(generation) {
                        return;
                    }
                    if let Ok(api::LayerStackOutcome::Applied(placed)) = placed {
                        stack = placed;
                    } else {
                        // The layer is back either way, and the step is
                        // undone: only its place is off.
                        toasts::toast_info(&format!(
                            "{name} is back, but on top of its stack: it could not be put where it was"
                        ));
                        self.resync(generation).await;
                        if !self.is_current(generation) {
                            return;
                        }
                    }
                }
                self.journal.update(|journal| {
                    journal.remap_layer(&zone_id, layer.id, made.id);
                    journal.settle(
                        id,
                        Settled::Unapplied(StudioEdit::LayerRemoved(Box::new(RemovedLayer {
                            zone_id: zone_id.clone(),
                            name,
                            index,
                            layer: made,
                        }))),
                    );
                });
                self.announce_replay();
                self.adopt_written(&zone_id, stack);
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }

    fn remove_again(self, id: EntryId, removed: RemovedLayer) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let found = locate(&scene, &removed.zone_id, removed.layer.id).and_then(|(zone, layer)| {
            let index = zone.layers.iter().position(|held| held.id == layer.id)?;
            Some((zone.id.to_string(), index, layer))
        });
        let Some((zone_id, index, layer)) = found else {
            self.drop_step(
                id,
                true,
                &format!("{} is no longer in this scene", removed.name),
            );
            return;
        };
        let Some(generation) = self.begin() else {
            return;
        };
        leptos::task::spawn_local(async move {
            let fence = Fence::of(&scene);
            let result = api::delete_layer(&zone_id, &layer.id.to_string(), Some(&fence)).await;
            if !self.is_current(generation) {
                return;
            }
            if let Some(stack) = self.replayed_stack(generation, id, true, result).await {
                self.journal.update(|journal| {
                    journal.settle(
                        id,
                        Settled::Applied(StudioEdit::LayerRemoved(Box::new(RemovedLayer {
                            zone_id: zone_id.clone(),
                            name: removed.name,
                            index,
                            layer,
                        }))),
                    );
                });
                self.announce_replay();
                self.adopt_written(&zone_id, stack);
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }

    /// Undo or redo the reorder at the journal place `id`, onto a stack
    /// that still holds the layers it put in order (see
    /// [`replayable_order`]).
    pub(super) fn replay_layers_reordered(
        self,
        id: EntryId,
        redo: bool,
        zone_id: String,
        name: String,
        before: Vec<SceneLayerId>,
        after: Vec<SceneLayerId>,
    ) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let order = scene
            .zones
            .iter()
            .find(|zone| zone.id.to_string() == zone_id)
            .and_then(|zone| {
                replayable_order(&order_of(&zone.layers), if redo { &after } else { &before })
            });
        let Some(order) = order else {
            self.drop_step(
                id,
                redo,
                "the layers it put in order are no longer the ones in this zone",
            );
            return;
        };
        let Some(generation) = self.begin() else {
            return;
        };
        leptos::task::spawn_local(async move {
            let fence = Fence::of(&scene);
            let result = api::reorder_layers(&zone_id, order, Some(&fence)).await;
            if !self.is_current(generation) {
                return;
            }
            if let Some(stack) = self.replayed_stack(generation, id, redo, result).await {
                let entry = StudioEdit::LayersReordered {
                    zone_id: zone_id.clone(),
                    name,
                    before,
                    after,
                };
                let settled = if redo {
                    Settled::Applied(entry)
                } else {
                    Settled::Unapplied(entry)
                };
                self.journal.update(|journal| journal.settle(id, settled));
                self.announce_replay();
                self.adopt_written(&zone_id, stack);
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }
}
