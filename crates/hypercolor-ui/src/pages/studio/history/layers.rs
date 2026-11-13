//! Layers added as one step.
//!
//! One face on the screens of several zones is one layer per zone, written
//! in sequence because each write is fenced by the scene revision the one
//! before it left. They are still one thing somebody did, so they are one
//! journal entry: Undo removes every layer it made and Redo makes them
//! again, each as it stood when it was removed.

use hypercolor_types::api::scene::CreateLayerRequest;
use hypercolor_types::layer::{LayerScope, SceneLayer, SceneLayerId};
use leptos::prelude::*;

use super::{EntryId, Fence, Settled, StudioEdit, StudioHistory, Waiting, refuses_step};
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

/// How an add came out, for whoever asked: the names it landed on and the
/// names it did not, and the outputs the new layers are limited to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddOutcome {
    pub landed: Vec<String>,
    pub failed: Vec<String>,
    pub outputs: Vec<String>,
}

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
    pub fn add_layers(self, name: String, adds: Vec<LayerAdd>, done: Callback<AddOutcome>) {
        self.when_free(Waiting::AddLayers { name, adds, done });
    }

    pub(super) fn add_layers_now(
        self,
        name: String,
        adds: Vec<LayerAdd>,
        done: Callback<AddOutcome>,
    ) {
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
            let _ = done.try_run(outcome);
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
                        &format!("{name} and the screens it was on are gone"),
                    ));
                } else if finished {
                    self.announce_replay();
                }
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }
}
