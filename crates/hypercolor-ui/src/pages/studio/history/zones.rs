//! Zone structure in Studio history. Creating, changing, and deleting a
//! zone each record one journal entry, so Ctrl+Z takes it back and
//! Ctrl+Shift+Z puts it back, in order with every other edit.

use std::collections::HashSet;

use hypercolor_types::api::scene::{
    CreateLayerRequest, EditMembersRequest, MemberEdit, MemberState, PatchZoneRequest,
};
use hypercolor_types::layer::SceneLayer;
use hypercolor_types::scene::{SceneId, ZoneId, ZoneRole};
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
    pub brightness: f32,
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
            brightness: zone.brightness,
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

/// Whether renaming `zone` to `draft` would change nothing: a blank draft,
/// or one that matches the name its tab shows. A legacy "Default zone"
/// shows as "Main", so leaving "Main" as it is renames nothing.
#[must_use]
pub fn rename_is_noop(zone: &api::ZoneResource, draft: &str) -> bool {
    let draft = draft.trim();
    draft.is_empty()
        || draft == zone.name
        || draft == crate::zones::surface::zone_display_name(&zone.name, zone.role)
}

/// The tab that takes the selection when the light zone `zone_id` goes:
/// the one before it, else the one after. `None` when it is the only one.
#[must_use]
pub fn light_neighbor(scene: &api::SceneDocument, zone_id: &str) -> Option<String> {
    let lights: Vec<String> = scene
        .zones
        .iter()
        .filter(|zone| zone.role != ZoneRole::Display)
        .map(|zone| zone.id.to_string())
        .collect();
    let index = lights.iter().position(|id| id == zone_id)?;
    index
        .checked_sub(1)
        .and_then(|before| lights.get(before))
        .or_else(|| lights.get(index + 1))
        .cloned()
}

/// How creating a zone, and the move into it, came out.
pub(crate) enum NewZone {
    /// The zone was not made, or a scene switch overtook it: there is
    /// nothing in this scene to show.
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

/// The live scene, read fresh between the steps of a multi-request zone
/// operation. A scene activated elsewhere since the operation began is
/// refused, so no step lands outside the scene it started in.
async fn live_scene(scene_id: SceneId) -> Result<api::SceneDocument, &'static str> {
    match api::fetch_active_scene().await {
        Ok(scene) if scene.id == scene_id => Ok(scene),
        Ok(_) => Err("another scene is active now"),
        Err(_) => Err("the scene could not be read"),
    }
}

/// Put complete output snapshots back into a zone in one transaction.
async fn restore_outputs(
    scene_id: SceneId,
    zone_id: ZoneId,
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

/// Patch one zone of the scene `scene_id` against its live revision.
/// Reports whether it applied.
async fn patch_live_zone(scene_id: SceneId, zone_id: &str, request: &PatchZoneRequest) -> bool {
    let Ok(live) = live_scene(scene_id).await else {
        return false;
    };
    matches!(
        api::zones::update_zone(zone_id, request, live.revision).await,
        Ok(api::zones::ZoneOutcome::Applied(_))
    )
}

/// How taking a zone down came out.
enum TakeDown {
    /// The zone and its outputs are gone.
    Deleted,
    /// The zone stays as it was: nothing changed, or what changed was put
    /// back.
    Kept,
    /// The outputs left, but the delete failed and they could not be put
    /// back: the release that stranded them, for the journal to undo.
    Stranded(Vec<MemberEdit>),
}

/// Why undoing a zone's creation left the zone in place.
enum Removal {
    Removed,
    /// The zone holds something new, so it stays for good.
    Kept(&'static str),
    /// The delete did not go through; trying again may.
    Failed(String),
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

    /// Release the scene after a zone write. Zone replies carry no
    /// revision, so the scene is read back first, and whatever waits next
    /// writes against the scene as this left it.
    pub(super) async fn release(self, generation: u64) {
        self.resync(generation).await;
        self.refresh.run(());
        self.finish(generation);
    }

    /// Tell a zone create a scene switch overtook, while Studio is still
    /// there to hear it.
    fn overtaken(self, outcome: Box<dyn FnOnce(NewZone)>) {
        if self.alive() {
            outcome(NewZone::NotCreated);
        }
    }

    /// Move the selection to `next` when it is on `zone_id`, a zone
    /// history just removed, so nothing goes on reading a zone that is gone.
    pub(super) fn select_away(self, zone_id: &str, next: Option<String>) {
        if self.selected.try_get_untracked().flatten().as_deref() == Some(zone_id) {
            self.selected.set(next);
        }
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
        self.announce_replay();
    }

    /// Create a zone. `created` hears the zone, or `None` when it was not
    /// made. Undo deletes it again while it is still empty.
    pub fn create_zone(
        self,
        name: String,
        color: Option<String>,
        created: Box<dyn FnOnce(Option<api::ZoneResource>)>,
    ) {
        self.when_free(super::Waiting::CreateZone {
            name,
            color,
            plan: None,
            outcome: Box::new(move |outcome| {
                created(match outcome {
                    NewZone::Created { zone, .. } => Some(zone),
                    NewZone::NotCreated => None,
                });
            }),
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
        self.when_free(super::Waiting::CreateZone {
            name,
            color,
            plan: Some(plan),
            outcome,
        });
    }

    pub(super) fn create_zone_now(
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
                self.overtaken(outcome);
                return;
            }
            let zone = match created {
                Ok(api::zones::ZoneOutcome::Applied(zone)) => zone,
                Ok(api::zones::ZoneOutcome::Stale { .. }) => {
                    toasts::toast_error("Scene changed elsewhere. Reloaded, try again.");
                    outcome(NewZone::NotCreated);
                    self.release(generation).await;
                    return;
                }
                Err(error) => {
                    toasts::toast_error(&format!("Zone create failed: {error}"));
                    outcome(NewZone::NotCreated);
                    self.release(generation).await;
                    return;
                }
            };
            let mut moved = Moved::Unchanged;
            let mut filled = Vec::new();
            if let Some(plan) = plan {
                let live = live_scene(scene.id).await;
                if !self.is_current(generation) {
                    self.overtaken(outcome);
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
                                Sent::Superseded => {
                                    self.overtaken(outcome);
                                    return;
                                }
                            }
                        }
                    }
                    Err(reason) => {
                        toasts::toast_error(&format!(
                            "Created {}, but the move failed: {reason}",
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
            self.release(generation).await;
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
        self.when_free(super::Waiting::PatchZone {
            zone_id,
            after,
            failure,
        });
    }

    pub(super) fn patch_zone_now(
        self,
        zone_id: String,
        after: PatchZoneRequest,
        failure: &'static str,
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
            self.release(generation).await;
        });
    }

    /// Delete a zone. Its outputs leave first, through one membership
    /// transaction whose receipt holds their complete snapshots, so an
    /// undo can put them back exactly; then the empty zone goes.
    /// When the deleted zone held the selection, the light tab beside it
    /// takes it, chosen as the delete runs so a queued delete never hands
    /// it to a zone an earlier one removed. `deleted` runs with the
    /// restore record once the zone is gone; the same record is the
    /// journal entry Ctrl+Z restores.
    pub fn delete_zone(self, zone_id: String, deleted: Callback<DeletedZone>) {
        self.when_free(super::Waiting::DeleteZone { zone_id, deleted });
    }

    pub(super) fn delete_zone_now(self, zone_id: String, deleted: Callback<DeletedZone>) {
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
        let neighbor = light_neighbor(&scene, &zone_id);
        let Some(generation) = self.begin() else {
            return;
        };
        spawn_local(async move {
            let outcome = self.take_down_zone(scene.id, &mut record).await;
            if self.is_current(generation) {
                match outcome {
                    TakeDown::Deleted => {
                        self.select_away(&zone_id, neighbor);
                        self.journal.update(|journal| {
                            journal.record(StudioEdit::ZoneDeleted(Box::new(record.clone())));
                        });
                        let _ = deleted.try_run(record);
                    }
                    // Undo is how those devices get back.
                    TakeDown::Stranded(released) => self.journal.update(|journal| {
                        journal.record(StudioEdit::Members(released));
                    }),
                    TakeDown::Kept => {}
                }
            }
            self.release(generation).await;
        });
    }

    /// Release a zone's outputs and delete it, refreshing `record` from the
    /// live zone and filling `record.outputs` with complete snapshots. A
    /// failed delete puts the outputs back; when even that fails, the
    /// release comes back as [`TakeDown::Stranded`], so the devices always
    /// have a way home.
    async fn take_down_zone(self, scene_id: SceneId, record: &mut DeletedZone) -> TakeDown {
        let scene = match live_scene(scene_id).await {
            Ok(scene) => scene,
            Err(reason) => {
                toasts::toast_error(&format!("Zone delete failed: {reason}"));
                return TakeDown::Kept;
            }
        };
        let Some((index, zone)) = scene
            .zones
            .iter()
            .enumerate()
            .find(|(_, zone)| zone.id.to_string() == record.zone_id)
        else {
            toasts::toast_error("Zone delete failed: the zone no longer exists");
            return TakeDown::Kept;
        };
        *record = DeletedZone::of(zone, index, record.token);
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
        let mut revision = scene.revision;
        let mut release = Vec::new();
        if !changes.is_empty() {
            let request = EditMembersRequest {
                scene_id: scene.id,
                changes,
                assignment: None,
            };
            match api::zones::edit_members(&request, revision).await {
                Ok(api::zones::ZoneOutcome::Applied(response)) => {
                    revision = response.document.revision;
                    release.clone_from(&response.changes);
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
                    return TakeDown::Kept;
                }
                Err(error) => {
                    toasts::toast_error(&format!("Zone delete failed: {error}"));
                    return TakeDown::Kept;
                }
            }
        }
        let outcome = api::zones::delete_zone(&record.zone_id, revision).await;
        if matches!(outcome, Ok(api::zones::ZoneOutcome::Applied(()))) {
            return TakeDown::Deleted;
        }
        let put_back = record.outputs.is_empty()
            || match live_scene(scene_id).await {
                Ok(live) => restore_outputs(live.id, zone_key, &record.outputs, live.revision)
                    .await
                    .is_ok(),
                Err(_) => false,
            };
        if !put_back {
            toasts::toast_error(&format!(
                "{} was not deleted, and its devices could not be put back. Undo returns them.",
                record.name
            ));
            return TakeDown::Stranded(release);
        }
        match outcome {
            Err(error) => toasts::toast_error(&format!("Zone delete failed: {error}")),
            _ => toasts::toast_error("Scene changed elsewhere. Reloaded, try again."),
        }
        TakeDown::Kept
    }

    /// Rebuild a deleted zone in the scene `scene_id`: its name, color,
    /// outputs (those still unclaimed), layers, paused state, brightness,
    /// and, best effort, its position. Returns the zone it made, or `None`
    /// when it made none or a scene switch overtook it partway.
    async fn rebuild_zone(
        self,
        generation: u64,
        scene_id: SceneId,
        record: &DeletedZone,
    ) -> Option<api::ZoneResource> {
        let live = match live_scene(scene_id).await {
            Ok(live) => live,
            Err(reason) => {
                toasts::toast_error(&format!("Zone restore failed: {reason}"));
                return None;
            }
        };
        if !self.is_current(generation) {
            return None;
        }
        let created =
            api::zones::create_zone(&record.name, record.color.as_deref(), live.revision).await;
        if !self.is_current(generation) {
            return None;
        }
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

        if !record.outputs.is_empty() {
            let live = live_scene(scene_id).await;
            if !self.is_current(generation) {
                return None;
            }
            if let Ok(live) = live {
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
                let put_back = free.is_empty()
                    || restore_outputs(live.id, zone_id, &free, live.revision)
                        .await
                        .is_ok();
                if !self.is_current(generation) {
                    return None;
                }
                if !put_back {
                    problems.push("its devices could not be put back");
                } else if moved_on > 0 {
                    problems.push("devices you moved since stayed where they are");
                }
            } else {
                problems.push("its devices could not be put back");
            }
        }

        if !record.layers.is_empty() {
            let live = live_scene(scene_id).await;
            if !self.is_current(generation) {
                return None;
            }
            let mut revision = live.ok().map(|live| live.revision);
            let mut layers_back = revision.is_some();
            if layers_back {
                for request in restore_layer_requests(record) {
                    let result = api::create_layer(&zone_key, &request, revision).await;
                    if !self.is_current(generation) {
                        return None;
                    }
                    if let Ok(api::LayerStackOutcome::Applied(stack)) = result {
                        revision = Some(stack.revision);
                    } else {
                        layers_back = false;
                        break;
                    }
                }
            }
            if !layers_back {
                problems.push("its layers could not all be put back");
            }
        }

        let restyle = PatchZoneRequest {
            enabled: (!record.enabled).then_some(false),
            brightness: ((record.brightness - zone.brightness).abs() > f32::EPSILON)
                .then_some(record.brightness),
            ..Default::default()
        };
        if restyle.enabled.is_some() || restyle.brightness.is_some() {
            let patched = patch_live_zone(scene_id, &zone_key, &restyle).await;
            if !self.is_current(generation) {
                return None;
            }
            if !patched {
                problems.push(match (restyle.enabled, restyle.brightness) {
                    (Some(_), Some(_)) => "it could not be paused or dimmed again",
                    (Some(_), None) => "it could not be paused again",
                    _ => "its brightness could not be put back",
                });
            }
        }

        let live = live_scene(scene_id).await;
        if !self.is_current(generation) {
            return None;
        }
        let index = live.ok().and_then(|live| {
            let current = live.zones.iter().position(|zone| zone.id == zone_id)?;
            (current != record.index).then_some(record.index)
        });
        if let Some(index) = index {
            let request = PatchZoneRequest {
                index: Some(index),
                ..Default::default()
            };
            let placed = patch_live_zone(scene_id, &zone_key, &request).await;
            if !self.is_current(generation) {
                return None;
            }
            if !placed {
                problems.push("it kept its new place at the end of the tabs");
            }
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
        self.when_free(super::Waiting::UndoDeletion(token));
    }

    pub(super) fn undo_deletion_now(self, token: u64) {
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
                            Some(StudioEdit::ZoneDeleted(record)) => Some(record.as_ref().clone()),
                            _ => None,
                        });
                if let Some(record) = record {
                    self.prepare_words(&StudioEdit::ZoneDeleted(Box::new(record.clone())), false);
                    self.replay_zone_deleted(index, false, record, Settle::Remove);
                }
            }
            None => {}
        }
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
        let Some(scene) = self.current_scene() else {
            return;
        };
        let scene_id = scene.id;
        let neighbor = light_neighbor(&scene, &record.zone_id);
        let Some(generation) = self.begin() else {
            return;
        };
        spawn_local(async move {
            if redo {
                let outcome = self.take_down_zone(scene_id, &mut record).await;
                if self.is_current(generation) {
                    match outcome {
                        TakeDown::Deleted => {
                            self.select_away(&record.zone_id, neighbor);
                            self.settle_zone_entry(
                                index,
                                redo,
                                StudioEdit::ZoneDeleted(Box::new(record)),
                                settle,
                            );
                        }
                        // The redo only got as far as releasing the devices;
                        // the step becomes that release, so undo returns them.
                        TakeDown::Stranded(released) => {
                            self.replay_words.try_set_value(None);
                            self.settle_zone_entry(
                                index,
                                redo,
                                StudioEdit::Members(released),
                                settle,
                            );
                        }
                        TakeDown::Kept => {}
                    }
                }
            } else if let Some(zone) = self.rebuild_zone(generation, scene_id, &record).await
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
            self.release(generation).await;
        });
    }

    /// Undo or redo a rename, recolor, pause, or reorder.
    pub(super) fn replay_zone_patched(self, redo: bool, zone_id: String, patch: PatchZoneRequest) {
        let Some(scene_id) = self.current_scene().map(|scene| scene.id) else {
            return;
        };
        let Some(generation) = self.begin() else {
            return;
        };
        spawn_local(async move {
            if patch_live_zone(scene_id, &zone_id, &patch).await {
                if self.is_current(generation) {
                    self.journal.update(|journal| journal.complete(redo));
                    self.announce_replay();
                }
            } else if self.is_current(generation) {
                toasts::toast_error("The zone changed elsewhere; history was not applied");
            }
            self.release(generation).await;
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
        let Some(scene) = self.current_scene() else {
            return;
        };
        let scene_id = scene.id;
        let neighbor = light_neighbor(&scene, &zone_id);
        let Some(generation) = self.begin() else {
            return;
        };
        spawn_local(async move {
            let old = zone_id.clone();
            let settled = if redo {
                self.recreate_zone(generation, scene_id, name, color, filled)
                    .await
                    .map(|(entry, remade)| (entry, Some(remade)))
            } else {
                self.uncreate_zone(generation, scene_id, zone_id, neighbor, name, color, filled)
                    .await
                    .map(|entry| (entry, None))
            };
            if let Some((entry, remade)) = settled
                && self.is_current(generation)
            {
                // A kept zone with nothing moved back leaves no step to
                // redo, so its entry goes rather than eating a keypress.
                let settle = if matches!(&entry, StudioEdit::Members(changes) if changes.is_empty())
                {
                    Settle::Remove
                } else {
                    Settle::Step
                };
                self.settle_zone_entry(index, redo, entry, settle);
                if let Some(new) = remade {
                    self.journal.update(|journal| journal.remap_zone(&old, new));
                }
            }
            self.release(generation).await;
        });
    }

    /// The undo of a zone's creation. `None` leaves the entry where it is,
    /// for a step that changed nothing; otherwise the entry the journal
    /// keeps in its place. The selection follows the devices back to the
    /// zone they came from, or moves to `neighbor` when the zone goes and
    /// nothing went back into a zone.
    #[allow(clippy::too_many_arguments)]
    async fn uncreate_zone(
        self,
        generation: u64,
        scene_id: SceneId,
        zone_id: String,
        neighbor: Option<String>,
        name: String,
        color: Option<String>,
        filled: Vec<MemberEdit>,
    ) -> Option<StudioEdit> {
        let moves_out = !filled.is_empty();
        let mut filled_now = filled.clone();
        let mut home = None;
        if moves_out {
            let scene = match live_scene(scene_id).await {
                Ok(scene) => scene,
                Err(reason) => {
                    toasts::toast_error(&format!("History could not be applied: {reason}"));
                    return None;
                }
            };
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
                    home = changes
                        .iter()
                        .find_map(|change| change.after.as_ref())
                        .map(|state| state.zone_id.to_string());
                    filled_now = changes.into_iter().map(flip).collect();
                }
                Sent::Failed | Sent::Superseded => return None,
            }
        }
        // The devices are back where they came from; only the zone is left.
        let live = live_scene(scene_id).await;
        if !self.is_current(generation) {
            return None;
        }
        let removal = match live {
            Err(reason) => Removal::Failed(reason.to_owned()),
            Ok(live) => match live
                .zones
                .iter()
                .find(|zone| zone.id.to_string() == zone_id)
            {
                None => Removal::Removed,
                Some(zone) if !zone.members.is_empty() || !zone.layers.is_empty() => {
                    Removal::Kept("it holds devices or layers now")
                }
                Some(_) => match api::zones::delete_zone(&zone_id, live.revision).await {
                    Ok(api::zones::ZoneOutcome::Applied(())) => Removal::Removed,
                    Ok(api::zones::ZoneOutcome::Stale { .. }) => {
                        Removal::Failed("the scene changed elsewhere".to_owned())
                    }
                    Err(error) => Removal::Failed(error.to_string()),
                },
            },
        };
        // Everything below reads Studio state, which a scene switch or an
        // unmount during the delete has replaced or disposed.
        if !self.is_current(generation) {
            return None;
        }
        match (&home, &removal) {
            (Some(home), _) => self.selected.set(Some(home.clone())),
            (None, Removal::Removed) => self.select_away(&zone_id, neighbor),
            (None, _) => {}
        }
        match removal {
            Removal::Removed => Some(StudioEdit::ZoneCreated {
                zone_id,
                name,
                color,
                filled: filled_now,
            }),
            Removal::Kept(reason) => {
                let kept = format!("Kept {name}: {reason}");
                toasts::toast_info(&kept);
                if filled_now.is_empty() {
                    self.replay_words.try_set_value(Some(kept));
                } else {
                    self.prepare_words(&StudioEdit::Members(filled_now.clone()), false);
                }
                Some(StudioEdit::Members(filled_now))
            }
            Removal::Failed(reason) if !moves_out => {
                toasts::toast_error(&format!("{name} could not be removed: {reason}"));
                None
            }
            Removal::Failed(reason) => {
                toasts::toast_info(&format!(
                    "Moved its devices back, but {name} could not be removed: {reason}"
                ));
                self.prepare_words(&StudioEdit::Members(filled_now.clone()), false);
                Some(StudioEdit::Members(filled_now))
            }
        }
    }

    async fn recreate_zone(
        self,
        generation: u64,
        scene_id: SceneId,
        name: String,
        color: Option<String>,
        filled: Vec<MemberEdit>,
    ) -> Option<(StudioEdit, ZoneId)> {
        let live = match live_scene(scene_id).await {
            Ok(live) => live,
            Err(reason) => {
                toasts::toast_error(&format!("History could not be applied: {reason}"));
                return None;
            }
        };
        if !self.is_current(generation) {
            return None;
        }
        let created = api::zones::create_zone(&name, color.as_deref(), live.revision).await;
        if !self.is_current(generation) {
            return None;
        }
        let zone = match created {
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
        let moves_in = !filled.is_empty();
        if moves_in {
            let scene = live_scene(scene_id).await.ok();
            if !self.is_current(generation) {
                return None;
            }
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
                    match self
                        .send_members(generation, request, scene.revision, None)
                        .await
                    {
                        Sent::Landed { changes, .. } => filled_now = changes,
                        Sent::Failed => {}
                        Sent::Superseded => return None,
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
        let entry = StudioEdit::ZoneCreated {
            zone_id: zone.id.to_string(),
            name: zone.name,
            color: zone.color,
            filled: filled_now,
        };
        if moves_in && matches!(&entry, StudioEdit::ZoneCreated { filled, .. } if filled.is_empty())
        {
            // The devices did not come along, so the redo only made the zone.
            self.prepare_words(&entry, true);
        }
        Some((entry, zone.id))
    }
}
