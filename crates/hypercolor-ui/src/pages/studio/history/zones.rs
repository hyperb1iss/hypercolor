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
use super::held::{HeldWrite, Pending, Resume, Then, Unresolved};
use super::{
    EntryId, Fence, MovePlan, Sent, Settled, StudioEdit, StudioHistory, Written, edit_members,
    flip, prepare_members_replay, written,
};
use crate::components::layout_builder::keyed::{self, Answer};
use crate::components::layout_builder::writer;
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
pub(super) async fn live_scene(scene_id: SceneId) -> Result<api::SceneDocument, &'static str> {
    match api::fetch_active_scene_promptly().await {
        Ok(scene) if scene.id == scene_id => Ok(scene),
        Ok(_) => Err("another scene is active now"),
        Err(_) => Err("the scene could not be read"),
    }
}

/// Create a zone fenced at `fence`, under an idempotency key of its own.
async fn create_zone(name: &str, color: Option<&str>, fence: &Fence) -> Written<api::ZoneResource> {
    let key = keyed::new_key();
    written(fence, key.clone(), || {
        api::zones::create_zone(name, color, fence, &key)
    })
    .await
}

/// Patch a zone fenced at `fence`, under an idempotency key of its own.
async fn update_zone(
    zone_id: &str,
    request: &PatchZoneRequest,
    fence: &Fence,
) -> Written<api::ZoneResource> {
    let key = keyed::new_key();
    written(fence, key.clone(), || {
        api::zones::update_zone_keyed(zone_id, request, fence, &key)
    })
    .await
}

/// Delete a zone fenced at `fence`, under `key`.
async fn delete_zone(zone_id: &str, fence: &Fence, key: &str) -> Answer<()> {
    keyed::send(&fence.run, || api::zones::delete_zone(zone_id, fence, key)).await
}

/// How one write of a multi-step zone operation came out.
enum Wrote {
    Landed,
    /// The scene could not be read, so nothing went out.
    Unread(&'static str),
    /// The daemon refused it, for this reason.
    Refused(String),
    /// No answer said whether it landed: the fence and key it went out
    /// under, to settle it with.
    Unknown(Fence, String),
}

/// Put complete output snapshots back into a zone in one transaction.
async fn restore_outputs(
    scene_id: SceneId,
    zone_id: ZoneId,
    outputs: &[Output],
    fence: &Fence,
) -> Wrote {
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
    match edit_members(&request, fence).await {
        Written::Answered(Ok(api::zones::ZoneOutcome::Applied(_))) => Wrote::Landed,
        Written::Answered(Ok(api::zones::ZoneOutcome::Stale { .. })) => {
            Wrote::Refused("the scene changed elsewhere".to_owned())
        }
        Written::Answered(Err(error)) => Wrote::Refused(error.to_string()),
        Written::Unknown(fence, key) => Wrote::Unknown(fence, key),
    }
}

/// Patch one zone of the scene `scene_id` against its live revision.
async fn patch_live_zone(scene_id: SceneId, zone_id: &str, request: &PatchZoneRequest) -> Wrote {
    let live = match live_scene(scene_id).await {
        Ok(live) => live,
        Err(reason) => return Wrote::Unread(reason),
    };
    match update_zone(zone_id, request, &Fence::of(&live)).await {
        Written::Answered(Ok(api::zones::ZoneOutcome::Applied(_))) => Wrote::Landed,
        Written::Answered(Ok(api::zones::ZoneOutcome::Stale { .. })) => {
            Wrote::Refused("the scene changed elsewhere".to_owned())
        }
        Written::Answered(Err(error)) => Wrote::Refused(error.to_string()),
        Written::Unknown(fence, key) => Wrote::Unknown(fence, key),
    }
}

/// The zone `zone_id` names in the live scene `scene_id`: `Err` when the
/// scene cannot be read, `Ok(None)` when the zone is not in it.
async fn live_zone(
    scene_id: SceneId,
    zone_id: Option<&str>,
) -> Result<Option<api::ZoneResource>, &'static str> {
    let live = live_scene(scene_id).await?;
    Ok(zone_id.and_then(|zone_id| {
        live.zones
            .into_iter()
            .find(|zone| zone.id.to_string() == zone_id)
    }))
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
    /// No answer came back for the release of its devices or for the
    /// delete itself: what went out, held until it settles.
    Unknown(Box<Pending>),
}

/// Why undoing a zone's creation left the zone in place.
enum Removal {
    Removed,
    /// The zone holds something new, so it stays for good.
    Kept(&'static str),
    /// The delete did not go through; trying again may.
    Failed(String),
    /// No answer came back for the delete, which went out under this fence
    /// and key.
    Unknown(Fence, String),
}

impl StudioHistory {
    /// A fresh token for a deletion or a held place in the journal.
    pub(super) fn next_token(self) -> u64 {
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

    /// Settle the journal place `id` once a replayed zone step is done:
    /// `entry` is the step it holds now, applied after a redo and not after
    /// an undo, unless `settle` drops the place.
    fn settle_zone_entry(self, id: EntryId, redo: bool, entry: StudioEdit, settle: Settle) {
        let settled = match settle {
            Settle::Step if redo => Settled::Applied(entry),
            Settle::Step => Settled::Unapplied(entry),
            Settle::Remove => Settled::Gone,
        };
        self.journal.update(|journal| journal.settle(id, settled));
        self.announce_replay();
    }

    /// Settle a zone's creation the undo or redo at the journal place `id`
    /// left as `entry`. A kept zone with nothing moved back leaves no step
    /// to redo, so its place goes rather than eat a keypress.
    fn settle_created(self, id: EntryId, redo: bool, entry: StudioEdit) {
        let settle = if matches!(&entry, StudioEdit::Members(changes) if changes.is_empty()) {
            Settle::Remove
        } else {
            Settle::Step
        };
        self.settle_zone_entry(id, redo, entry, settle);
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
            let created = create_zone(&name, color.as_deref(), &Fence::of(&scene)).await;
            if !self.is_current(generation) {
                self.overtaken(outcome);
                return;
            }
            let zone = match created {
                Written::Answered(Ok(api::zones::ZoneOutcome::Applied(zone))) => zone,
                Written::Answered(Ok(api::zones::ZoneOutcome::Stale { .. })) => {
                    toasts::toast_error("Scene changed elsewhere. Reloaded, try again.");
                    outcome(NewZone::NotCreated);
                    self.release(generation).await;
                    return;
                }
                Written::Answered(Err(error)) => {
                    toasts::toast_error(&format!("Zone create failed: {error}"));
                    outcome(NewZone::NotCreated);
                    self.release(generation).await;
                    return;
                }
                Written::Unknown(fence, key) => {
                    self.record_held(
                        scene.id,
                        format!("creating {name}"),
                        Pending {
                            fence,
                            key,
                            write: HeldWrite::Create {
                                name: name.clone(),
                                color: color.clone(),
                            },
                        },
                    );
                    outcome(NewZone::NotCreated);
                    self.release(generation).await;
                    return;
                }
            };
            let mut moved = Moved::Unchanged;
            let mut filled = Vec::new();
            let mut pending = None;
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
                                Sent::Unknown(unanswered) => {
                                    toasts::toast_info(&format!(
                                        "Created {}, but no answer came back for the move, so it may not have happened. Check the canvas.",
                                        zone.name
                                    ));
                                    moved = Moved::Failed;
                                    pending = Some(unanswered);
                                }
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
            self.record_created_zone(scene.id, &zone, filled, pending);
            outcome(NewZone::Created { zone, moved });
            self.release(generation).await;
        });
    }

    /// Record the creation and keep an unanswered fill in the same place
    /// until its receipt says what the operation must undo.
    pub(super) fn record_created_zone(
        self,
        scene_id: SceneId,
        zone: &api::ZoneResource,
        filled: Vec<MemberEdit>,
        pending: Option<Box<Pending>>,
    ) {
        let id = self.journal.try_update(|journal| {
            journal.record(StudioEdit::ZoneCreated {
                zone_id: zone.id.to_string(),
                name: zone.name.clone(),
                color: zone.color.clone(),
                filled,
            })
        });
        if let (Some(id), Some(pending)) = (id, pending) {
            let name = &zone.name;
            self.hold_place(
                scene_id,
                id,
                format!("creating {name}"),
                *pending,
                Then::Fill {
                    zone_id: zone.id.to_string(),
                    name: zone.name.clone(),
                    color: zone.color.clone(),
                },
            );
        }
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
                self.send_members(generation, request, Fence::of(scene), point)
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
                self.send_members(generation, request, Fence::of(scene), None)
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
        let what = format!("the change to {}", zone.name);
        let Some(generation) = self.begin() else {
            return;
        };
        spawn_local(async move {
            let result = update_zone(&zone_id, &after, &Fence::of(&scene)).await;
            if !self.is_current(generation) {
                return;
            }
            match result {
                Written::Answered(Ok(api::zones::ZoneOutcome::Applied(_))) => {
                    self.journal.update(|journal| {
                        journal.record(StudioEdit::ZonePatched {
                            zone_id,
                            before,
                            after,
                        });
                    });
                }
                Written::Answered(Ok(api::zones::ZoneOutcome::Stale { .. })) => {
                    toasts::toast_error("Scene changed elsewhere. Reloaded, try again.");
                }
                Written::Answered(Err(error)) => {
                    toasts::toast_error(&describe_zone_failure(failure, &error));
                }
                Written::Unknown(fence, key) => self.record_held(
                    scene.id,
                    what,
                    Pending {
                        fence,
                        key,
                        write: HeldWrite::Patch {
                            zone_id,
                            before,
                            after,
                        },
                    },
                ),
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
                    TakeDown::Unknown(pending) => {
                        let name = format!("deleting {}", record.name);
                        let held = name.clone();
                        let entry = self.journal.try_update(|journal| {
                            journal.record(StudioEdit::Unresolved { name: held })
                        });
                        if let Some(entry) = entry {
                            self.hold(scene.id, entry, name, *pending, Then::Record);
                        }
                    }
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
            let through = writer::edit_mark();
            match edit_members(&request, &Fence::of(&scene)).await {
                Written::Answered(Ok(api::zones::ZoneOutcome::Applied(response))) => {
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
                Written::Answered(Ok(api::zones::ZoneOutcome::Stale { .. })) => {
                    toasts::toast_error("Scene changed elsewhere. Reloaded, try again.");
                    return TakeDown::Kept;
                }
                Written::Answered(Err(error)) => {
                    toasts::toast_error(&format!("Zone delete failed: {error}"));
                    return TakeDown::Kept;
                }
                // The devices may have left already: the release waits, so
                // that whatever it did, Undo can put them back.
                Written::Unknown(fence, key) => {
                    toasts::toast_info(&format!(
                        "Lost the connection while deleting {}. Its devices may have left it already; once the connection is back, Undo can put them back.",
                        record.name
                    ));
                    return TakeDown::Unknown(Box::new(Pending {
                        fence,
                        key,
                        write: HeldWrite::Members { request, through },
                    }));
                }
            }
        }
        let fence = Fence {
            revision,
            run: scene.run_epoch.clone(),
        };
        let key = keyed::new_key();
        let outcome = match delete_zone(&record.zone_id, &fence, &key).await {
            Answer::Final {
                outcome: Ok(api::zones::ZoneOutcome::Applied(())),
                ..
            } => return TakeDown::Deleted,
            Answer::Final { outcome, .. } => Some(outcome),
            Answer::OtherRun | Answer::Forgotten => None,
            // No answer even under its key: the record waits, whole, until
            // the delete can go out again and say what it did.
            Answer::Lost(_) => {
                toasts::toast_info(&format!(
                    "Lost the connection while deleting {}. Once it is back, Undo can bring back what was removed.",
                    record.name
                ));
                return TakeDown::Unknown(Box::new(Pending {
                    fence,
                    key,
                    write: HeldWrite::Delete {
                        record: record.clone(),
                        release,
                    },
                }));
            }
        };
        let live = live_scene(scene_id).await;
        // A zone gone from the live scene was deleted, by this delete or
        // another, and its record can rebuild it. When the daemon restarted
        // while the delete was out, the run that ended commits nothing more,
        // and when it let the delete's answer go, the delete was done: either
        // way this read is the whole story.
        if let Ok(live) = &live
            && !live.zones.iter().any(|zone| zone.id == zone_key)
        {
            return TakeDown::Deleted;
        }
        // A restart with no scene to read yet: the record waits, whole, for
        // the next read to say what the delete did.
        if outcome.is_none() && live.is_err() {
            return TakeDown::Unknown(Box::new(Pending {
                fence,
                key,
                write: HeldWrite::Delete {
                    record: record.clone(),
                    release,
                },
            }));
        }
        let put_back = record.outputs.is_empty()
            || match live {
                Ok(live) => matches!(
                    restore_outputs(live.id, zone_key, &record.outputs, &Fence::of(&live)).await,
                    Wrote::Landed
                ),
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
            Some(Err(error)) => toasts::toast_error(&format!("Zone delete failed: {error}")),
            None => toasts::toast_error("Zone delete did not go through"),
            Some(Ok(_)) => toasts::toast_error("Scene changed elsewhere. Reloaded, try again."),
        }
        TakeDown::Kept
    }

    /// Undo a zone's deletion: make the zone again from `record`, then put
    /// the rest back ([`Self::finish_rebuild`]). Returns the zone it made,
    /// or `None` when it made none, when a scene switch overtook it
    /// partway, or when no answer said whether the zone was made: the
    /// create then holds the journal place `id` until it settles, and the
    /// rest follows once it is known to have landed (`remove` says the
    /// place goes then, for a restore from the Undo bar).
    async fn rebuild_zone(
        self,
        generation: u64,
        scene_id: SceneId,
        record: &DeletedZone,
        id: EntryId,
        remove: bool,
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
        let created = create_zone(&record.name, record.color.as_deref(), &Fence::of(&live)).await;
        if !self.is_current(generation) {
            return None;
        }
        let zone = match created {
            Written::Answered(Ok(api::zones::ZoneOutcome::Applied(zone))) => zone,
            Written::Answered(Ok(api::zones::ZoneOutcome::Stale { .. })) => {
                toasts::toast_error("Scene changed elsewhere; the zone was not restored.");
                return None;
            }
            Written::Answered(Err(error)) => {
                toasts::toast_error(&format!("Zone restore failed: {error}"));
                return None;
            }
            // Its place keeps the deletion until the create settles, so a
            // second Undo cannot make the zone twice.
            Written::Unknown(fence, key) => {
                self.replay_words.try_set_value(None);
                self.hold_replay(
                    scene_id,
                    id,
                    format!("restoring {}", record.name),
                    Pending {
                        fence,
                        key,
                        write: HeldWrite::Create {
                            name: record.name.clone(),
                            color: record.color.clone(),
                        },
                    },
                    Then::Resume(Box::new(Resume::Rebuild {
                        record: record.clone(),
                        remove,
                    })),
                );
                return None;
            }
        };
        self.finish_rebuild(generation, scene_id, record, zone)
            .await
    }

    /// Put the rest of a deleted zone back into `zone`, the zone made again
    /// for it: its outputs (those still unclaimed), layers, paused state,
    /// brightness, and, best effort, its position. Returns the zone, or
    /// `None` when a scene switch overtook it partway.
    async fn finish_rebuild(
        self,
        generation: u64,
        scene_id: SceneId,
        record: &DeletedZone,
        zone: api::ZoneResource,
    ) -> Option<api::ZoneResource> {
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
                let put_back = if free.is_empty() {
                    Wrote::Landed
                } else {
                    restore_outputs(live.id, zone_id, &free, &Fence::of(&live)).await
                };
                if !self.is_current(generation) {
                    return None;
                }
                match put_back {
                    Wrote::Landed if moved_on > 0 => {
                        problems.push("devices you moved since stayed where they are");
                    }
                    Wrote::Landed => {}
                    Wrote::Unknown(..) => problems.push("its devices may not be back"),
                    Wrote::Unread(_) | Wrote::Refused(_) => {
                        problems.push("its devices could not be put back");
                    }
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
            let mut fence = live.ok().map(|live| Fence::of(&live));
            // Whether the layers all came back; `None` when no answer said.
            let mut layers_back = Some(fence.is_some());
            if let Some(at) = fence.as_mut() {
                for request in restore_layer_requests(record) {
                    let key = keyed::new_key();
                    let answer = keyed::send(&at.run, || {
                        api::create_layer_keyed(&zone_key, &request, at, &key)
                    })
                    .await;
                    if !self.is_current(generation) {
                        return None;
                    }
                    match answer {
                        Answer::Final {
                            outcome: Ok(api::LayerStackOutcome::Applied(stack)),
                            ..
                        } => at.revision = stack.revision,
                        Answer::Final { .. } => {
                            layers_back = Some(false);
                            break;
                        }
                        Answer::OtherRun | Answer::Forgotten | Answer::Lost(_) => {
                            layers_back = None;
                            break;
                        }
                    }
                }
            }
            match layers_back {
                Some(true) => {}
                Some(false) => problems.push("its layers could not all be put back"),
                None => problems.push("its layers may not all be back"),
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
            let unknown = matches!(patched, Wrote::Unknown(..));
            if !matches!(patched, Wrote::Landed) {
                problems.push(match (restyle.enabled, restyle.brightness, unknown) {
                    (Some(_), Some(_), false) => "it could not be paused or dimmed again",
                    (Some(_), Some(_), true) => "it may not be paused or dimmed again",
                    (Some(_), None, false) => "it could not be paused again",
                    (Some(_), None, true) => "it may not be paused again",
                    (None, _, false) => "its brightness could not be put back",
                    (None, _, true) => "its brightness may not be back",
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
            match placed {
                Wrote::Landed => {}
                Wrote::Unknown(..) => {
                    problems.push("it may have kept its new place at the end of the tabs")
                }
                Wrote::Unread(_) | Wrote::Refused(_) => {
                    problems.push("it kept its new place at the end of the tabs");
                }
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

    /// Settle the journal place `id` once the deletion `record` describes
    /// was undone into the zone `zone`: the place holds the deletion of
    /// that zone, ready to redo (or goes, with `remove`), every entry that
    /// named the old zone names the new one, and the selection follows.
    fn settle_rebuilt(self, id: EntryId, mut record: DeletedZone, zone: ZoneId, remove: bool) {
        let old = std::mem::replace(&mut record.zone_id, zone.to_string());
        self.selected.set(Some(record.zone_id.clone()));
        let settle = if remove { Settle::Remove } else { Settle::Step };
        self.settle_zone_entry(id, false, StudioEdit::ZoneDeleted(Box::new(record)), settle);
        self.journal
            .update(|journal| journal.remap_zone(&old, zone));
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
                .map(|id| (id, journal.is_latest(id)))
        });
        match found {
            Some((_, true)) => {
                let recorded = self.journal.with_untracked(super::StudioJournal::recorded);
                self.replay_now(false, recorded);
            }
            Some((id, false)) => {
                let record = self
                    .journal
                    .with_untracked(|journal| match journal.edit(id) {
                        Some(StudioEdit::ZoneDeleted(record)) => Some(record.as_ref().clone()),
                        _ => None,
                    });
                if let Some(record) = record {
                    self.prepare_words(&StudioEdit::ZoneDeleted(Box::new(record.clone())), false);
                    self.replay_zone_deleted(id, false, record, Settle::Remove);
                }
            }
            None => {}
        }
    }

    /// Undo puts the deleted zone at the journal place `id` back; redo
    /// deletes it again.
    pub(super) fn replay_zone_deleted_step(self, id: EntryId, redo: bool, record: DeletedZone) {
        self.replay_zone_deleted(id, redo, record, Settle::Step);
    }

    fn replay_zone_deleted(self, id: EntryId, redo: bool, mut record: DeletedZone, settle: Settle) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let scene_id = scene.id;
        let neighbor = light_neighbor(&scene, &record.zone_id);
        let Some(generation) = self.begin() else {
            return;
        };
        let remove = settle == Settle::Remove;
        spawn_local(async move {
            if redo {
                let entry = StudioEdit::ZoneDeleted(Box::new(record.clone()));
                let outcome = self.take_down_zone(scene_id, &mut record).await;
                if self.is_current(generation) {
                    match outcome {
                        TakeDown::Deleted => {
                            self.select_away(&record.zone_id, neighbor);
                            self.settle_zone_entry(
                                id,
                                redo,
                                StudioEdit::ZoneDeleted(Box::new(record)),
                                settle,
                            );
                        }
                        // The redo only got as far as releasing the devices;
                        // the step becomes that release, so undo returns them.
                        TakeDown::Stranded(released) => {
                            self.replay_words.try_set_value(None);
                            self.settle_zone_entry(id, redo, StudioEdit::Members(released), settle);
                        }
                        // The redo's place holds until what it did is known,
                        // so it cannot run twice.
                        TakeDown::Unknown(pending) => {
                            self.replay_words.try_set_value(None);
                            self.hold_place(
                                scene_id,
                                id,
                                format!("deleting {}", record.name),
                                *pending,
                                Then::Replay { entry, redo },
                            );
                        }
                        TakeDown::Kept => {}
                    }
                }
            } else if let Some(zone) = self
                .rebuild_zone(generation, scene_id, &record, id, remove)
                .await
                && self.is_current(generation)
            {
                self.settle_rebuilt(id, record, zone.id, remove);
            }
            self.release(generation).await;
        });
    }

    /// Undo or redo the rename, recolor, pause, or reorder at the journal
    /// place `id`, which set `after` over `before`.
    pub(super) fn replay_zone_patched(
        self,
        id: EntryId,
        redo: bool,
        zone_id: String,
        before: PatchZoneRequest,
        after: PatchZoneRequest,
    ) {
        let Some(scene_id) = self.current_scene().map(|scene| scene.id) else {
            return;
        };
        let Some(generation) = self.begin() else {
            return;
        };
        let entry = StudioEdit::ZonePatched {
            zone_id: zone_id.clone(),
            before: before.clone(),
            after: after.clone(),
        };
        let (from, to) = if redo {
            (before, after)
        } else {
            (after, before)
        };
        spawn_local(async move {
            let patched = patch_live_zone(scene_id, &zone_id, &to).await;
            if self.is_current(generation) {
                match patched {
                    Wrote::Landed => self.settle_zone_entry(id, redo, entry, Settle::Step),
                    Wrote::Unread(reason) => {
                        toasts::toast_error(&format!("History could not be applied: {reason}"));
                    }
                    Wrote::Refused(reason) => {
                        toasts::toast_error(&format!("History could not be applied: {reason}"));
                    }
                    // Its place keeps the change until the patch settles.
                    Wrote::Unknown(fence, key) => {
                        self.replay_words.try_set_value(None);
                        let what = if redo {
                            "redoing the zone change"
                        } else {
                            "undoing the zone change"
                        };
                        self.hold_replay(
                            scene_id,
                            id,
                            what.to_owned(),
                            Pending {
                                fence,
                                key,
                                write: HeldWrite::Patch {
                                    zone_id,
                                    before: from,
                                    after: to,
                                },
                            },
                            Then::Replay { entry, redo },
                        );
                    }
                }
            }
            self.release(generation).await;
        });
    }

    /// Undo of the zone Studio made at the journal place `id`: its devices
    /// go back where they came from, then the zone goes if nothing else is
    /// in it. A zone that gained devices or layers since stays, and the
    /// entry becomes its move alone. Redo makes the zone again and moves
    /// the devices back in.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn replay_zone_created(
        self,
        id: EntryId,
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
            let settled = if redo {
                self.recreate_zone(generation, scene_id, id, zone_id, name, color, filled)
                    .await
            } else {
                self.uncreate_zone(
                    generation, scene_id, id, zone_id, neighbor, name, color, filled,
                )
                .await
            };
            if let Some(entry) = settled
                && self.is_current(generation)
            {
                self.settle_created(id, redo, entry);
            }
            self.release(generation).await;
        });
    }

    /// The undo of a zone's creation. `None` leaves the place `id` as it
    /// is: for a step that changed nothing, or one held there until a write
    /// whose answer was lost settles. Otherwise the entry the journal keeps
    /// in its place. The selection follows the devices back to the zone
    /// they came from, or moves to `neighbor` when the zone goes and
    /// nothing went back into a zone.
    #[allow(clippy::too_many_arguments)]
    async fn uncreate_zone(
        self,
        generation: u64,
        scene_id: SceneId,
        id: EntryId,
        zone_id: String,
        neighbor: Option<String>,
        name: String,
        color: Option<String>,
        filled: Vec<MemberEdit>,
    ) -> Option<StudioEdit> {
        if filled.is_empty() {
            return self
                .finish_uncreate(
                    generation,
                    scene_id,
                    id,
                    Uncreating {
                        zone_id,
                        neighbor,
                        name,
                        color,
                        filled_now: Vec::new(),
                        home: None,
                        moved_out: false,
                    },
                )
                .await;
        }
        let scene = match live_scene(scene_id).await {
            Ok(scene) => scene,
            Err(reason) => {
                toasts::toast_error(&format!("History could not be applied: {reason}"));
                return None;
            }
        };
        let changes: Vec<MemberEdit> = filled.iter().cloned().map(flip).collect();
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
            .send_members(generation, request, Fence::of(&scene), None)
            .await
        {
            Sent::Landed { changes, .. } => {
                let home = changes
                    .iter()
                    .find_map(|change| change.after.as_ref())
                    .map(|state| state.zone_id.to_string());
                let filled_now = changes.into_iter().map(flip).collect();
                self.finish_uncreate(
                    generation,
                    scene_id,
                    id,
                    Uncreating {
                        zone_id,
                        neighbor,
                        name,
                        color,
                        filled_now,
                        home,
                        moved_out: true,
                    },
                )
                .await
            }
            // Its place keeps the zone's creation until the move settles;
            // the zone goes after it, once the move is known to have landed.
            Sent::Unknown(pending) => {
                self.replay_words.try_set_value(None);
                self.hold_replay(
                    scene_id,
                    id,
                    format!("undoing {name}"),
                    *pending,
                    Then::Resume(Box::new(Resume::Uncreate {
                        zone_id,
                        neighbor,
                        name,
                        color,
                        filled,
                    })),
                );
                None
            }
            Sent::Failed | Sent::Superseded => None,
        }
    }

    /// The rest of a zone creation's undo, once its devices are back where
    /// they came from: the zone goes, if nothing else is in it.
    async fn finish_uncreate(
        self,
        generation: u64,
        scene_id: SceneId,
        id: EntryId,
        step: Uncreating,
    ) -> Option<StudioEdit> {
        let Uncreating {
            zone_id,
            neighbor,
            name,
            color,
            filled_now,
            home,
            moved_out,
        } = step;
        let live = live_scene(scene_id).await;
        if !self.is_current(generation) {
            return None;
        }
        let mut doomed = None;
        let removal = match live {
            Err(reason) => Removal::Failed(reason.to_owned()),
            Ok(live) => match live
                .zones
                .iter()
                .enumerate()
                .find(|(_, zone)| zone.id.to_string() == zone_id)
            {
                None => Removal::Removed,
                Some((_, zone)) if !zone.members.is_empty() || !zone.layers.is_empty() => {
                    Removal::Kept("it holds devices or layers now")
                }
                Some((index, zone)) => {
                    doomed = Some(DeletedZone::of(zone, index, self.next_token()));
                    let key = keyed::new_key();
                    let fence = Fence::of(&live);
                    match delete_zone(&zone_id, &fence, &key).await {
                        Answer::Final {
                            outcome: Ok(api::zones::ZoneOutcome::Applied(())),
                            ..
                        } => Removal::Removed,
                        Answer::Final {
                            outcome: Ok(api::zones::ZoneOutcome::Stale { .. }),
                            ..
                        } => Removal::Failed("the scene changed elsewhere".to_owned()),
                        Answer::Final {
                            outcome: Err(error),
                            ..
                        } => Removal::Failed(error.to_string()),
                        Answer::OtherRun | Answer::Forgotten | Answer::Lost(_) => {
                            Removal::Unknown(fence, key)
                        }
                    }
                }
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
        let created = |filled| StudioEdit::ZoneCreated {
            zone_id: zone_id.clone(),
            name: name.clone(),
            color: color.clone(),
            filled,
        };
        match removal {
            Removal::Removed => Some(created(filled_now)),
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
            // Its place holds until the delete settles: gone, it is the
            // zone's creation, ready to redo; still there, it is the move
            // alone when devices went back, and the creation still applied
            // when none did.
            Removal::Unknown(fence, key) => {
                self.replay_words.try_set_value(None);
                let not = if moved_out {
                    Settled::Unapplied(StudioEdit::Members(filled_now.clone()))
                } else {
                    Settled::Applied(created(filled_now.clone()))
                };
                if let Some(record) = doomed {
                    self.hold_replay(
                        scene_id,
                        id,
                        format!("removing {name}"),
                        Pending {
                            fence,
                            key,
                            write: HeldWrite::Delete {
                                record,
                                release: Vec::new(),
                            },
                        },
                        Then::Either {
                            landed: Settled::Unapplied(created(filled_now)),
                            not,
                        },
                    );
                }
                None
            }
            Removal::Failed(reason) if !moved_out => {
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

    /// The redo of a zone's creation at the journal place `id`: the zone is
    /// made again, every entry that named the old one names the new one,
    /// and its devices follow ([`Self::finish_recreate`]). `None` when it
    /// made nothing, or when a write whose answer was lost holds the place
    /// until it settles; otherwise the entry the journal keeps there.
    #[allow(clippy::too_many_arguments)]
    async fn recreate_zone(
        self,
        generation: u64,
        scene_id: SceneId,
        id: EntryId,
        old: String,
        name: String,
        color: Option<String>,
        filled: Vec<MemberEdit>,
    ) -> Option<StudioEdit> {
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
        let created = create_zone(&name, color.as_deref(), &Fence::of(&live)).await;
        if !self.is_current(generation) {
            return None;
        }
        let zone = match created {
            Written::Answered(Ok(api::zones::ZoneOutcome::Applied(zone))) => zone,
            Written::Answered(Ok(api::zones::ZoneOutcome::Stale { .. })) => {
                toasts::toast_error("Scene changed elsewhere; history was not applied");
                return None;
            }
            Written::Answered(Err(error)) => {
                toasts::toast_error(&format!("History could not be applied: {error}"));
                return None;
            }
            // Its place keeps the creation until the create settles, so a
            // second Redo cannot make the zone twice.
            Written::Unknown(fence, key) => {
                self.replay_words.try_set_value(None);
                self.hold_replay(
                    scene_id,
                    id,
                    format!("making {name} again"),
                    Pending {
                        fence,
                        key,
                        write: HeldWrite::Create {
                            name: name.clone(),
                            color: color.clone(),
                        },
                    },
                    Then::Resume(Box::new(Resume::Recreate {
                        zone_id: old,
                        name,
                        color,
                        filled,
                    })),
                );
                return None;
            }
        };
        self.journal
            .update(|journal| journal.remap_zone(&old, zone.id));
        self.finish_recreate(generation, scene_id, id, zone, filled)
            .await
    }

    /// The rest of a zone creation's redo, once the zone is made again as
    /// `zone`: the devices it was filled with move back in.
    async fn finish_recreate(
        self,
        generation: u64,
        scene_id: SceneId,
        id: EntryId,
        zone: api::ZoneResource,
        filled: Vec<MemberEdit>,
    ) -> Option<StudioEdit> {
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
                        .send_members(generation, request, Fence::of(&scene), None)
                        .await
                    {
                        Sent::Landed { changes, .. } => filled_now = changes,
                        Sent::Failed => {}
                        // The zone is made; its place holds until the move
                        // settles, then keeps what the move moved.
                        Sent::Unknown(pending) => {
                            self.selected.set(Some(zone.id.to_string()));
                            self.replay_words.try_set_value(None);
                            self.hold_replay(
                                scene_id,
                                id,
                                format!("moving the devices back into {}", zone.name),
                                *pending,
                                Then::Fill {
                                    zone_id: zone.id.to_string(),
                                    name: zone.name,
                                    color: zone.color,
                                },
                            );
                            return None;
                        }
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
        Some(entry)
    }

    /// Go on with the undo or redo of zone structure that `item` held, now
    /// its first write is known to have landed: `did` is the entry that
    /// undoes what that write did. A scene that cannot be read right now
    /// leaves it held for the next try.
    pub(super) fn resume_now(self, item: Unresolved, did: StudioEdit) {
        if self
            .current_scene()
            .is_none_or(|scene| scene.id != item.scene_id)
        {
            return;
        }
        let Then::Resume(resume) = item.then.clone() else {
            return;
        };
        let Some(generation) = self.begin() else {
            return;
        };
        let (scene_id, id) = (item.scene_id, item.entry);
        spawn_local(async move {
            let made = match &did {
                StudioEdit::ZoneCreated { zone_id, .. } => Some(zone_id.clone()),
                _ => None,
            };
            match *resume {
                Resume::Rebuild { record, remove } => {
                    match live_zone(scene_id, made.as_deref()).await {
                        // The journal this resumes in is gone: a scene switch came
                        // while the scene was read.
                        _ if !self.is_current(generation) => {}
                        Err(_) => self.hold_again(item, did),
                        Ok(None) => {
                            toasts::toast_info(&format!(
                                "{} was made again, but it is gone now.",
                                record.name
                            ));
                            self.journal.update(|journal| {
                                journal.settle(
                                    id,
                                    Settled::Applied(StudioEdit::ZoneDeleted(Box::new(record))),
                                );
                            });
                        }
                        Ok(Some(zone)) => {
                            if let Some(zone) = self
                                .finish_rebuild(generation, scene_id, &record, zone)
                                .await
                                && self.is_current(generation)
                            {
                                self.settle_rebuilt(id, record, zone.id, remove);
                            }
                        }
                    }
                }
                Resume::Uncreate {
                    zone_id,
                    neighbor,
                    name,
                    color,
                    ..
                } => {
                    let receipt = match did {
                        StudioEdit::Members(changes) => changes,
                        _ => Vec::new(),
                    };
                    let home = receipt
                        .iter()
                        .find_map(|change| change.after.as_ref())
                        .map(|state| state.zone_id.to_string());
                    let step = Uncreating {
                        zone_id,
                        neighbor,
                        name,
                        color,
                        filled_now: receipt.into_iter().map(flip).collect(),
                        home,
                        moved_out: true,
                    };
                    if let Some(entry) = self.finish_uncreate(generation, scene_id, id, step).await
                        && self.is_current(generation)
                    {
                        self.settle_created(id, false, entry);
                    }
                }
                Resume::Recreate {
                    zone_id: old,
                    name,
                    color,
                    filled,
                } => match live_zone(scene_id, made.as_deref()).await {
                    // The journal this resumes in is gone: a scene switch came
                    // while the scene was read.
                    _ if !self.is_current(generation) => {}
                    Err(_) => self.hold_again(item, did),
                    Ok(None) => {
                        toasts::toast_info(&format!("{name} was made again, but it is gone now."));
                        self.journal.update(|journal| {
                            journal.settle(
                                id,
                                Settled::Unapplied(StudioEdit::ZoneCreated {
                                    zone_id: old,
                                    name,
                                    color,
                                    filled,
                                }),
                            );
                        });
                    }
                    Ok(Some(zone)) => {
                        self.journal
                            .update(|journal| journal.remap_zone(&old, zone.id));
                        if let Some(entry) = self
                            .finish_recreate(generation, scene_id, id, zone, filled)
                            .await
                            && self.is_current(generation)
                        {
                            self.settle_created(id, true, entry);
                        }
                    }
                },
            }
            self.release(generation).await;
        });
    }
}

/// Where a zone creation's undo stands once its devices are back.
struct Uncreating {
    zone_id: String,
    neighbor: Option<String>,
    name: String,
    color: Option<String>,
    /// The move into the zone, as it stands after the devices went back.
    filled_now: Vec<MemberEdit>,
    /// The zone the devices went back to, for the selection.
    home: Option<String>,
    /// Whether devices went back at all.
    moved_out: bool,
}
