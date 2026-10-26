//! Zone structure in Studio history. Creating, changing, and deleting a
//! zone each record one journal entry, so Ctrl+Z takes it back and
//! Ctrl+Shift+Z puts it back, in order with every other edit.

use std::collections::HashSet;

use hypercolor_types::api::scene::{
    CreateLayerRequest, EditMembersRequest, MemberEdit, MemberState, PatchZoneRequest,
};
use hypercolor_types::layer::SceneLayer;
use hypercolor_types::spatial::Output;
use leptos::prelude::*;
use leptos::task::spawn_local;

use super::super::tab_model::describe_zone_failure;
use super::{MovePlan, Sent, StudioEdit, StudioHistory, prepare_members_replay};
use crate::{api, toasts};

/// What a zone delete removed, kept so its undo can rebuild the zone.
#[derive(Clone, Debug, PartialEq)]
pub struct DeletedZone {
    /// Tells this deletion apart from every other, so its Undo bar and its
    /// journal entry can never both restore it.
    pub token: u64,
    /// The zone's id while it exists. A restore mints a new one.
    pub zone_id: String,
    pub name: String,
    pub color: Option<String>,
    pub enabled: bool,
    /// The zone's position in the scene's full zone order.
    pub index: usize,
    /// Complete snapshots of the outputs the zone held, in zone order.
    pub outputs: Vec<Output>,
    /// The zone's layer stack, bottom to top, as the scene had it.
    pub layers: Vec<SceneLayer>,
}

impl DeletedZone {
    /// The record for deleting `zone` from position `index` of the scene's
    /// zone order. Its outputs fill in once the delete releases them.
    #[must_use]
    pub fn of(zone: &api::ZoneResource, index: usize, token: u64) -> Self {
        Self {
            token,
            zone_id: zone.id.to_string(),
            name: zone.name.clone(),
            color: zone.color.clone(),
            enabled: zone.enabled,
            index,
            outputs: Vec::new(),
            layers: zone.layers.clone(),
        }
    }
}

/// The requests that rebuild a deleted zone's layers, bottom to top, each
/// with its source, controls, blend, opacity, transform, and adjustments.
#[must_use]
pub fn restore_layer_requests(record: &DeletedZone) -> Vec<CreateLayerRequest> {
    record
        .layers
        .iter()
        .map(|layer| CreateLayerRequest::from(api::update_request_from_layer(layer)))
        .collect()
}

/// The fields `after` sets, as `zone` (at `index` in the scene's zone
/// order) has them now: what a patch's undo puts back.
#[must_use]
pub fn zone_patch_before(
    zone: &api::ZoneResource,
    index: usize,
    after: &PatchZoneRequest,
) -> PatchZoneRequest {
    PatchZoneRequest {
        name: after.name.as_ref().map(|_| zone.name.clone()),
        enabled: after.enabled.map(|_| zone.enabled),
        brightness: after.brightness.map(|_| zone.brightness),
        color: after.color.as_ref().map(|_| zone.color.clone()),
        index: after.index.map(|_| index),
    }
}

/// How creating a zone, and the move into it, came out.
pub(crate) enum NewZone {
    /// The zone was not made; nothing moved.
    NotCreated,
    /// The zone exists. The move landed, failed, or had nothing to change.
    Created {
        zone: api::ZoneResource,
        moved: Moved,
    },
}

pub(crate) enum Moved {
    Landed,
    Failed,
    Unchanged,
}

/// Decides the move into a zone just made, against the scene that holds it.
pub(crate) type PlanMove =
    Box<dyn FnOnce(&api::SceneDocument, &api::ZoneResource) -> Option<MovePlan>>;

/// How a replayed zone step settles its journal entry.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Settle {
    /// An ordinary undo or redo: step the cursor past the entry.
    Step,
    /// An out-of-order restore from the Undo bar: drop the entry.
    Remove,
}

/// The live scene's revision, read fresh between the steps of a
/// multi-request zone operation.
async fn live_revision() -> Option<u64> {
    api::fetch_active_scene()
        .await
        .ok()
        .map(|scene| scene.revision)
}

/// Put complete output snapshots back into a zone in one transaction.
async fn restore_outputs(
    scene_id: hypercolor_types::scene::SceneId,
    zone_id: hypercolor_types::scene::ZoneId,
    outputs: &[Output],
    revision: u64,
) -> Result<(), ()> {
    let request = EditMembersRequest {
        scene_id,
        changes: outputs
            .iter()
            .enumerate()
            .map(|(index, output)| MemberEdit {
                before: None,
                after: Some(MemberState {
                    zone_id,
                    output: output.clone(),
                    index,
                }),
            })
            .collect(),
        assignment: None,
    };
    match api::zones::edit_members(&request, revision).await {
        Ok(api::zones::ZoneOutcome::Applied(_)) => Ok(()),
        _ => Err(()),
    }
}

/// Patch one zone against the live revision. Reports whether it applied.
async fn patch_live_zone(zone_id: &str, request: &PatchZoneRequest) -> bool {
    let Some(revision) = live_revision().await else {
        return false;
    };
    matches!(
        api::zones::update_zone(zone_id, request, revision).await,
        Ok(api::zones::ZoneOutcome::Applied(_))
    )
}

fn flip(change: MemberEdit) -> MemberEdit {
    MemberEdit {
        before: change.after,
        after: change.before,
    }
}

impl StudioHistory {
    fn next_token(self) -> u64 {
        let token = self.tokens.get_value().wrapping_add(1);
        self.tokens.set_value(token);
        token
    }

    /// Write a replayed zone step's new state into its journal entry and
    /// settle the cursor.
    fn settle_zone_entry(self, index: usize, redo: bool, entry: StudioEdit, settle: Settle) {
        self.journal.update(|journal| {
            if index >= journal.entries.len() {
                return;
            }
            match settle {
                Settle::Step => {
                    journal.entries[index] = entry;
                    journal.complete(redo);
                }
                Settle::Remove => {
                    journal.entries.remove(index);
                    if index < journal.cursor {
                        journal.cursor -= 1;
                    }
                }
            }
        });
    }

    /// Create a zone. `created` hears the zone, or `None` when it was not
    /// made. Undo deletes it again while it is still empty.
    pub fn create_zone(
        self,
        name: String,
        color: Option<String>,
        created: Box<dyn FnOnce(Option<api::ZoneResource>)>,
    ) {
        self.run_when_free(move || {
            self.create_zone_now(
                name,
                color,
                None,
                Box::new(move |outcome| {
                    created(match outcome {
                        NewZone::Created { zone, .. } => Some(zone),
                        NewZone::NotCreated => None,
                    });
                }),
            );
        });
    }

    /// Create a zone and move something into it as one journal entry, so
    /// one undo takes back both. `plan` decides the move against the scene
    /// with the new zone in it; `outcome` hears exactly once how it went.
    pub fn create_zone_filled(
        self,
        name: String,
        color: Option<String>,
        plan: PlanMove,
        outcome: Box<dyn FnOnce(NewZone)>,
    ) {
        self.run_when_free(move || self.create_zone_now(name, color, Some(plan), outcome));
    }

    fn create_zone_now(
        self,
        name: String,
        color: Option<String>,
        plan: Option<PlanMove>,
        outcome: Box<dyn FnOnce(NewZone)>,
    ) {
        let Some(scene) = self.current_scene() else {
            outcome(NewZone::NotCreated);
            return;
        };
        let Some(generation) = self.begin() else {
            outcome(NewZone::NotCreated);
            return;
        };
        spawn_local(async move {
            let created = api::zones::create_zone(&name, color.as_deref(), scene.revision).await;
            if !self.is_current(generation) {
                if self.alive() {
                    outcome(NewZone::NotCreated);
                }
                return;
            }
            let zone = match created {
                Ok(api::zones::ZoneOutcome::Applied(zone)) => zone,
                Ok(api::zones::ZoneOutcome::Stale { .. }) => {
                    toasts::toast_error("Scene changed elsewhere. Reloaded, try again.");
                    outcome(NewZone::NotCreated);
                    self.refresh.run(());
                    self.finish(generation);
                    return;
                }
                Err(error) => {
                    toasts::toast_error(&format!("Zone create failed: {error}"));
                    outcome(NewZone::NotCreated);
                    self.refresh.run(());
                    self.finish(generation);
                    return;
                }
            };
            let mut moved = Moved::Unchanged;
            let mut filled = Vec::new();
            if let Some(plan) = plan {
                let live = api::fetch_active_scene().await;
                if !self.is_current(generation) {
                    return;
                }
                match live {
                    Ok(live) => {
                        self.adopt(live.clone());
                        if let Some(plan) = plan(&live, &zone) {
                            match self.send_plan(generation, &live, &zone, plan).await {
                                Sent::Landed { changes, .. } => {
                                    filled = changes;
                                    moved = Moved::Landed;
                                }
                                Sent::Failed => moved = Moved::Failed,
                                Sent::Superseded => return,
                            }
                        }
                    }
                    Err(error) => {
                        toasts::toast_error(&format!(
                            "Created {}, but the move failed: {error}",
                            zone.name
                        ));
                        moved = Moved::Failed;
                    }
                }
            }
            self.journal.update(|journal| {
                journal.record(StudioEdit::ZoneCreated {
                    zone_id: zone.id.to_string(),
                    name: zone.name.clone(),
                    color: zone.color.clone(),
                    filled,
                });
            });
            outcome(NewZone::Created { zone, moved });
            self.refresh.run(());
            self.finish(generation);
        });
    }

    /// Send a move plan into `zone`, a zone of `scene`.
    async fn send_plan(
        self,
        generation: u64,
        scene: &api::SceneDocument,
        zone: &api::ZoneResource,
        plan: MovePlan,
    ) -> Sent {
        match plan {
            MovePlan::Assign {
                device_id,
                segments,
                placements,
                point,
            } => {
                let request = EditMembersRequest {
                    scene_id: scene.id,
                    changes: Vec::new(),
                    assignment: Some(hypercolor_types::api::scene::MemberAssignmentTarget {
                        zone_id: zone.id,
                        device_id,
                        segments,
                        placements,
                    }),
                };
                self.send_members(generation, request, scene.revision, point)
                    .await
            }
            MovePlan::Edit(changes) => {
                let Ok(changes) = super::rebase_preconditions(scene, changes) else {
                    toasts::toast_error("Assignment changed elsewhere; nothing was changed");
                    return Sent::Failed;
                };
                let request = EditMembersRequest {
                    scene_id: scene.id,
                    changes,
                    assignment: None,
                };
                self.send_members(generation, request, scene.revision, None)
                    .await
            }
        }
    }

    /// Rename, recolor, pause or resume, or reorder a zone. Undo puts the
    /// changed fields back as they were.
    pub fn patch_zone(self, zone_id: String, after: PatchZoneRequest, failure: &'static str) {
        self.run_when_free(move || self.patch_zone_now(zone_id, after, failure));
    }

    fn patch_zone_now(self, zone_id: String, after: PatchZoneRequest, failure: &'static str) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some((index, zone)) = scene
            .zones
            .iter()
            .enumerate()
            .find(|(_, zone)| zone.id.to_string() == zone_id)
        else {
            return;
        };
        let before = zone_patch_before(zone, index, &after);
        if before == after {
            return;
        }
        let Some(generation) = self.begin() else {
            return;
        };
        spawn_local(async move {
            let result = api::zones::update_zone(&zone_id, &after, scene.revision).await;
            if !self.is_current(generation) {
                return;
            }
            match result {
                Ok(api::zones::ZoneOutcome::Applied(_)) => self.journal.update(|journal| {
                    journal.record(StudioEdit::ZonePatched {
                        zone_id,
                        before,
                        after,
                    });
                }),
                Ok(api::zones::ZoneOutcome::Stale { .. }) => {
                    toasts::toast_error("Scene changed elsewhere. Reloaded, try again.");
                }
                Err(error) => toasts::toast_error(&describe_zone_failure(failure, &error)),
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }

    /// Delete a zone. Its outputs leave first, through one membership
    /// transaction whose receipt holds their complete snapshots, so an
    /// undo can put them back exactly; then the empty zone goes.
    /// `select_after` takes the selection when the deleted zone held it.
    /// `deleted` runs with the restore record once the zone is gone; the
    /// same record is the journal entry Ctrl+Z restores.
    pub fn delete_zone(
        self,
        zone_id: String,
        select_after: Option<String>,
        deleted: Callback<DeletedZone>,
    ) {
        self.when_free(super::Waiting::DeleteZone {
            zone_id,
            select_after,
            deleted,
        });
    }

    pub(super) fn delete_zone_now(
        self,
        zone_id: String,
        select_after: Option<String>,
        deleted: Callback<DeletedZone>,
    ) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some((index, zone)) = scene
            .zones
            .iter()
            .enumerate()
            .find(|(_, zone)| zone.id.to_string() == zone_id)
        else {
            return;
        };
        let mut record = DeletedZone::of(zone, index, self.next_token());
        let Some(generation) = self.begin() else {
            return;
        };
        spawn_local(async move {
            if self.take_down_zone(&mut record).await && self.is_current(generation) {
                if self.selected.get_untracked().as_deref() == Some(zone_id.as_str()) {
                    self.selected.set(select_after);
                }
                self.journal.update(|journal| {
                    journal.record(StudioEdit::ZoneDeleted(Box::new(record.clone())));
                });
                let _ = deleted.try_run(record);
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }

    /// Release a zone's outputs and delete it, filling `record.outputs`
    /// with their complete snapshots. A failed delete puts the outputs
    /// back, so the zone's devices are never stranded.
    async fn take_down_zone(self, record: &mut DeletedZone) -> bool {
        let Ok(scene) = api::fetch_active_scene().await else {
            toasts::toast_error("Zone delete failed: the scene could not be read");
            return false;
        };
        let Some(zone) = scene
            .zones
            .iter()
            .find(|zone| zone.id.to_string() == record.zone_id)
        else {
            toasts::toast_error("Zone delete failed: the zone no longer exists");
            return false;
        };
        let zone_key = zone.id;
        let changes: Vec<MemberEdit> = api::zone_outputs(zone)
            .into_iter()
            .enumerate()
            .map(|(index, output)| MemberEdit {
                before: Some(MemberState {
                    zone_id: zone_key,
                    output,
                    index,
                }),
                after: None,
            })
            .collect();
        record.layers.clone_from(&zone.layers);
        let mut revision = scene.revision;
        if !changes.is_empty() {
            let request = EditMembersRequest {
                scene_id: scene.id,
                changes,
                assignment: None,
            };
            match api::zones::edit_members(&request, revision).await {
                Ok(api::zones::ZoneOutcome::Applied(response)) => {
                    revision = response.document.revision;
                    let mut released: Vec<_> = response
                        .changes
                        .into_iter()
                        .filter_map(|change| change.before)
                        .collect();
                    released.sort_by_key(|state| state.index);
                    record.outputs = released.into_iter().map(|state| state.output).collect();
                }
                Ok(api::zones::ZoneOutcome::Stale { .. }) => {
                    toasts::toast_error("Scene changed elsewhere. Reloaded, try again.");
                    return false;
                }
                Err(error) => {
                    toasts::toast_error(&format!("Zone delete failed: {error}"));
                    return false;
                }
            }
        }
        match api::zones::delete_zone(&record.zone_id, revision).await {
            Ok(api::zones::ZoneOutcome::Applied(())) => true,
            outcome => {
                if !record.outputs.is_empty()
                    && let Some(revision) = live_revision().await
                {
                    let _ = restore_outputs(scene.id, zone_key, &record.outputs, revision).await;
                }
                match outcome {
                    Err(error) => toasts::toast_error(&format!("Zone delete failed: {error}")),
                    _ => toasts::toast_error("Scene changed elsewhere. Reloaded, try again."),
                }
                false
            }
        }
    }

    /// Rebuild a deleted zone: its name, color, paused state, outputs
    /// (those still unclaimed), layers, and, best effort, its position.
    /// Returns the zone it made.
    async fn rebuild_zone(self, record: &DeletedZone) -> Option<api::ZoneResource> {
        let revision = live_revision().await?;
        let created =
            api::zones::create_zone(&record.name, record.color.as_deref(), revision).await;
        let zone = match created {
            Ok(api::zones::ZoneOutcome::Applied(zone)) => zone,
            Ok(api::zones::ZoneOutcome::Stale { .. }) => {
                toasts::toast_error("Scene changed elsewhere; the zone was not restored.");
                return None;
            }
            Err(error) => {
                toasts::toast_error(&format!("Zone restore failed: {error}"));
                return None;
            }
        };
        let zone_id = zone.id;
        let zone_key = zone_id.to_string();
        let mut problems = Vec::new();

        if let Ok(live) = api::fetch_active_scene().await {
            let claimed: HashSet<&str> = live
                .zones
                .iter()
                .flat_map(|zone| zone.members.iter())
                .map(|member| member.id.0.as_str())
                .collect();
            let free: Vec<_> = record
                .outputs
                .iter()
                .filter(|output| !claimed.contains(output.id.as_str()))
                .cloned()
                .collect();
            let moved_on = record.outputs.len() - free.len();
            if !free.is_empty()
                && restore_outputs(live.id, zone_id, &free, live.revision)
                    .await
                    .is_err()
            {
                problems.push("its devices could not be put back");
            } else if moved_on > 0 {
                problems.push("devices you moved since stayed where they are");
            }
        }

        let mut revision = live_revision().await;
        let mut layers_back = true;
        for request in restore_layer_requests(record) {
            match api::create_layer(&zone_key, &request, revision).await {
                Ok(api::LayerStackOutcome::Applied(stack)) => revision = Some(stack.revision),
                _ => {
                    layers_back = false;
                    break;
                }
            }
        }
        if !layers_back {
            problems.push("its layers could not all be put back");
        }

        if !record.enabled
            && !patch_live_zone(
                &zone_key,
                &PatchZoneRequest {
                    enabled: Some(false),
                    ..Default::default()
                },
            )
            .await
        {
            problems.push("it could not be paused again");
        }
        let index = api::fetch_active_scene().await.ok().and_then(|live| {
            let current = live.zones.iter().position(|zone| zone.id == zone_id)?;
            (current != record.index).then_some(record.index)
        });
        if let Some(index) = index
            && !patch_live_zone(
                &zone_key,
                &PatchZoneRequest {
                    index: Some(index),
                    ..Default::default()
                },
            )
            .await
        {
            problems.push("it kept its new place at the end of the tabs");
        }

        if problems.is_empty() {
            toasts::toast_success(&format!("Restored {}", record.name));
        } else {
            toasts::toast_info(&format!(
                "Restored {}, but {}",
                record.name,
                problems.join(" and ")
            ));
        }
        Some(zone)
    }

    /// Take back the deletion `token` names, from its Undo bar: an
    /// ordinary undo when it is the latest edit, otherwise a restore that
    /// drops its journal entry. A deletion already restored stays put.
    pub fn undo_deletion(self, token: u64) {
        self.run_when_free(move || {
            let found = self.journal.with_untracked(|journal| {
                journal
                    .undoable_deletion(token)
                    .map(|index| (index, index + 1 == journal.cursor))
            });
            match found {
                Some((_, true)) => {
                    let recorded = self.journal.with_untracked(super::StudioJournal::recorded);
                    self.replay_now(false, recorded);
                }
                Some((index, false)) => {
                    let record =
                        self.journal
                            .with_untracked(|journal| match journal.entries.get(index) {
                                Some(StudioEdit::ZoneDeleted(record)) => {
                                    Some(record.as_ref().clone())
                                }
                                _ => None,
                            });
                    if let Some(record) = record {
                        self.replay_zone_deleted(index, false, record, Settle::Remove);
                    }
                }
                None => {}
            }
        });
    }

    /// Undo puts a deleted zone back; redo deletes it again.
    pub(super) fn replay_zone_deleted_step(self, index: usize, redo: bool, record: DeletedZone) {
        self.replay_zone_deleted(index, redo, record, Settle::Step);
    }

    fn replay_zone_deleted(
        self,
        index: usize,
        redo: bool,
        mut record: DeletedZone,
        settle: Settle,
    ) {
        let Some(generation) = self.begin() else {
            return;
        };
        spawn_local(async move {
            if redo {
                if self.take_down_zone(&mut record).await && self.is_current(generation) {
                    self.settle_zone_entry(
                        index,
                        redo,
                        StudioEdit::ZoneDeleted(Box::new(record)),
                        settle,
                    );
                }
            } else if let Some(zone) = self.rebuild_zone(&record).await
                && self.is_current(generation)
            {
                let old = std::mem::replace(&mut record.zone_id, zone.id.to_string());
                self.selected.set(Some(record.zone_id.clone()));
                self.settle_zone_entry(
                    index,
                    redo,
                    StudioEdit::ZoneDeleted(Box::new(record)),
                    settle,
                );
                self.journal
                    .update(|journal| journal.remap_zone(&old, zone.id));
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }

    /// Undo or redo a rename, recolor, pause, or reorder.
    pub(super) fn replay_zone_patched(self, redo: bool, zone_id: String, patch: PatchZoneRequest) {
        let Some(generation) = self.begin() else {
            return;
        };
        spawn_local(async move {
            if patch_live_zone(&zone_id, &patch).await {
                if self.is_current(generation) {
                    self.journal.update(|journal| journal.complete(redo));
                }
            } else if self.is_current(generation) {
                toasts::toast_error("The zone changed elsewhere; history was not applied");
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }

    /// Undo of a zone Studio made: its devices go back where they came
    /// from, then the zone goes if nothing else is in it. A zone that
    /// gained devices or layers since stays, and the entry becomes its
    /// move alone. Redo makes the zone again and moves the devices back in.
    pub(super) fn replay_zone_created(
        self,
        index: usize,
        redo: bool,
        zone_id: String,
        name: String,
        color: Option<String>,
        filled: Vec<MemberEdit>,
    ) {
        let Some(generation) = self.begin() else {
            return;
        };
        spawn_local(async move {
            let old = zone_id.clone();
            let settled = if redo {
                self.recreate_zone(generation, name, color, filled)
                    .await
                    .map(|(entry, remade)| (entry, Some(remade)))
            } else {
                self.uncreate_zone(generation, zone_id, name, color, filled)
                    .await
                    .map(|entry| (entry, None))
            };
            if let Some((entry, remade)) = settled
                && self.is_current(generation)
            {
                self.settle_zone_entry(index, redo, entry, Settle::Step);
                if let Some(new) = remade {
                    self.journal.update(|journal| journal.remap_zone(&old, new));
                }
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }

    async fn uncreate_zone(
        self,
        generation: u64,
        zone_id: String,
        name: String,
        color: Option<String>,
        filled: Vec<MemberEdit>,
    ) -> Option<StudioEdit> {
        let mut filled_now = filled.clone();
        if !filled.is_empty() {
            let scene = api::fetch_active_scene().await.ok()?;
            let changes: Vec<MemberEdit> = filled.into_iter().map(flip).collect();
            let Ok(changes) = prepare_members_replay(&scene, changes) else {
                toasts::toast_error("Assignment changed elsewhere; history was not applied");
                return None;
            };
            let request = EditMembersRequest {
                scene_id: scene.id,
                changes,
                assignment: None,
            };
            match self
                .send_members(generation, request, scene.revision, None)
                .await
            {
                Sent::Landed { changes, .. } => {
                    filled_now = changes.into_iter().map(flip).collect();
                }
                Sent::Failed | Sent::Superseded => return None,
            }
        }
        let live = api::fetch_active_scene().await.ok()?;
        let untouched = match live
            .zones
            .iter()
            .find(|zone| zone.id.to_string() == zone_id)
        {
            None => true,
            Some(zone) => {
                zone.members.is_empty()
                    && zone.layers.is_empty()
                    && matches!(
                        api::zones::delete_zone(&zone_id, live.revision).await,
                        Ok(api::zones::ZoneOutcome::Applied(()))
                    )
            }
        };
        if untouched {
            Some(StudioEdit::ZoneCreated {
                zone_id,
                name,
                color,
                filled: filled_now,
            })
        } else {
            toasts::toast_info(&format!("Kept {name}: it holds devices or layers now"));
            Some(StudioEdit::Members(filled_now))
        }
    }

    async fn recreate_zone(
        self,
        generation: u64,
        name: String,
        color: Option<String>,
        filled: Vec<MemberEdit>,
    ) -> Option<(StudioEdit, hypercolor_types::scene::ZoneId)> {
        let revision = live_revision().await?;
        let zone = match api::zones::create_zone(&name, color.as_deref(), revision).await {
            Ok(api::zones::ZoneOutcome::Applied(zone)) => zone,
            Ok(api::zones::ZoneOutcome::Stale { .. }) => {
                toasts::toast_error("Scene changed elsewhere; history was not applied");
                return None;
            }
            Err(error) => {
                toasts::toast_error(&format!("History could not be applied: {error}"));
                return None;
            }
        };
        let mut filled_now = Vec::new();
        if !filled.is_empty() {
            let scene = api::fetch_active_scene().await.ok();
            let changes = scene.as_ref().and_then(|scene| {
                let changes = filled
                    .into_iter()
                    .map(|mut change| {
                        if let Some(after) = change.after.as_mut() {
                            after.zone_id = zone.id;
                        }
                        change
                    })
                    .collect();
                prepare_members_replay(scene, changes).ok()
            });
            match (scene, changes) {
                (Some(scene), Some(changes)) => {
                    let request = EditMembersRequest {
                        scene_id: scene.id,
                        changes,
                        assignment: None,
                    };
                    if let Sent::Landed { changes, .. } = self
                        .send_members(generation, request, scene.revision, None)
                        .await
                    {
                        filled_now = changes;
                    }
                }
                _ => toasts::toast_info(&format!(
                    "Made {} again, but its devices moved on since",
                    zone.name
                )),
            }
        }
        if self.is_current(generation) {
            self.selected.set(Some(zone.id.to_string()));
        }
        Some((
            StudioEdit::ZoneCreated {
                zone_id: zone.id.to_string(),
                name: zone.name,
                color: zone.color,
                filled: filled_now,
            },
            zone.id,
        ))
    }
}
