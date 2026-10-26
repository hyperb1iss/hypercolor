//! History's writes whose outcome a lost connection hid.
//!
//! Every write of Studio's history goes out under an idempotency key and
//! goes out again under that key while no answer comes back (see
//! [`crate::components::layout_builder::keyed`]). When even that gets no
//! answer, or another daemon run answers, the write may have landed or
//! not. It then holds its place in the journal
//! ([`StudioEdit::Unresolved`]) with everything needed to settle it, and
//! neither undo nor redo moves past it until the connection is back. In
//! its own daemon run it goes out again under its key, and the daemon's
//! answer says what it did (a replay of the first answer when the first
//! request landed, receipt and all). Once that run has ended, which
//! commits nothing more, or the daemon has let the first answer go, which
//! it does only once that request is done, a read of the scene says what
//! it can.

use hypercolor_types::api::scene::{EditMembersRequest, MemberEdit, PatchZoneRequest};
use hypercolor_types::scene::SceneId;
use leptos::prelude::*;

use super::super::placement::Footprint;
use super::zones::{DeletedZone, live_scene, zone_patch_before};
use super::{Fence, StudioEdit, StudioHistory, retire_placed};
use crate::api;
use crate::components::layout_builder::keyed::{self, Answer};
use crate::toasts;

/// A write of history's whose outcome is not known yet.
#[derive(Clone, Debug)]
pub(super) enum HeldWrite {
    /// A zone delete, with the zone's record (layers included) and the
    /// release of its devices that went before it.
    Delete {
        record: DeletedZone,
        release: Vec<MemberEdit>,
    },
    /// A membership edit, which went out at the edit mark `through`; its
    /// receipt is what the journal keeps.
    Members {
        request: EditMembersRequest,
        through: u64,
    },
    /// A zone create.
    Create { name: String, color: Option<String> },
    /// A zone patch, with the fields it changed as they were before.
    Patch {
        zone_id: String,
        before: PatchZoneRequest,
        after: PatchZoneRequest,
    },
}

/// A held write, and the fence and key it went out under.
#[derive(Clone, Debug)]
pub(super) struct Pending {
    pub fence: Fence,
    pub key: String,
    pub write: HeldWrite,
}

/// Whether held writes are being settled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Settling {
    #[default]
    Idle,
    Running,
    /// Running, and asked to go round once more when it is done.
    Again,
}

/// A held write and its place in the journal.
#[derive(Clone, Debug)]
pub(super) struct Unresolved {
    pub scene_id: SceneId,
    /// The token of its place in the journal.
    pub token: u64,
    pub pending: Pending,
}

/// Whether a write fenced in daemon run `fence_run` settles by going out
/// again under its key, with the daemon's answer saying what it did, while
/// the scene is read in run `live_run`. In any other run its own has ended
/// and commits nothing more, so the read itself says what it can. A daemon
/// that names no runs gets the read.
#[must_use]
pub fn settles_by_key(fence_run: &str, live_run: &str) -> bool {
    !fence_run.is_empty() && fence_run == live_run
}

/// What a held write's key can say about it now.
enum Told<T> {
    /// The daemon's own answer to the write: the first request's, replayed,
    /// or this one's, when the first never reached it.
    Answer(api::ApiResult<api::MutationOutcome<T>>),
    /// Only the scene can tell, as this read of it shows: the write's run
    /// has ended, which commits nothing more, or the daemon let the first
    /// answer go, which it does only once that request is done.
    Read(api::SceneDocument),
}

/// Ask what became of a write held under `fence` and its key, with the
/// scene `live` as read just now: in the write's own run, `attempt` sends
/// it again under its key. `None` while nothing can tell yet.
async fn told<T, F, Fut>(
    scene_id: SceneId,
    fence: &Fence,
    live: api::SceneDocument,
    attempt: F,
) -> Option<Told<T>>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = api::client::KeyedResponse<T>>,
{
    if !settles_by_key(&fence.run, &live.run_epoch) {
        return Some(Told::Read(live));
    }
    match keyed::resend(&fence.run, attempt).await {
        Answer::Final { outcome, .. } => Some(Told::Answer(outcome)),
        // The first request may have committed after `live` was read and
        // before its answer went, so a read from now tells.
        Answer::Forgotten => live_scene(scene_id).await.ok().map(Told::Read),
        Answer::OtherRun | Answer::Lost(_) => None,
    }
}

/// What a delete whose outcome was unknown turned out to be: a delete when
/// it landed, so Undo rebuilds the zone from its record; the release of
/// its devices when it did not, so Undo puts them back; nothing when
/// nothing changed.
#[must_use]
pub fn resolve_deletion(
    deleted: bool,
    record: &DeletedZone,
    release: &[MemberEdit],
) -> Option<StudioEdit> {
    if deleted {
        return Some(StudioEdit::ZoneDeleted(Box::new(record.clone())));
    }
    (!release.is_empty()).then(|| StudioEdit::Members(release.to_vec()))
}

/// Whether `scene` holds what a membership edit's `changes` make. A change
/// within one zone is a placement, which landed only if the output sits
/// where the change put it; a move to another zone landed if the output is
/// there, wherever the daemon placed it on arrival; a removal, if it is
/// gone from the zone it left. An empty list (an assignment, whose outputs
/// only the daemon names) tells nothing.
#[must_use]
pub fn members_landed(scene: &api::SceneDocument, changes: &[MemberEdit]) -> bool {
    let live = |zone_id, output_id: &str| {
        scene
            .zones
            .iter()
            .find(|zone| zone.id == zone_id)
            .and_then(|zone| {
                api::zone_outputs(zone)
                    .into_iter()
                    .find(|output| output.id == output_id)
            })
    };
    !changes.is_empty()
        && changes
            .iter()
            .all(|change| match (&change.before, &change.after) {
                (Some(before), Some(after)) if before.zone_id == after.zone_id => {
                    live(after.zone_id, &after.output.id).is_some_and(|output| {
                        Footprint::of(&output) == Footprint::of(&after.output)
                    })
                }
                (_, Some(after)) => live(after.zone_id, &after.output.id).is_some(),
                (Some(before), None) => live(before.zone_id, &before.output.id).is_none(),
                (None, None) => true,
            })
}

impl StudioHistory {
    /// Hold `pending`, whose outcome is unknown, in the journal place
    /// `token` names, until it settles.
    pub(super) fn hold(self, scene_id: SceneId, token: u64, pending: Pending) {
        self.unresolved.try_update_value(|unresolved| {
            unresolved.push(Unresolved {
                scene_id,
                token,
                pending,
            });
        });
    }

    /// Record a new journal place for `pending`, which `what` names, hold
    /// it there, and tell the user.
    pub(super) fn record_held(self, scene_id: SceneId, what: String, pending: Pending) {
        let token = self.next_token();
        toasts::toast_info(&format!(
            "No answer came back for {what}, so it may or may not have been saved. It settles once the connection is back, and Undo waits for it."
        ));
        self.journal.update(|journal| {
            journal.record(StudioEdit::Unresolved { token, name: what });
        });
        self.hold(scene_id, token, pending);
    }

    /// Whether a held write waits to settle.
    pub(super) fn has_unresolved(self) -> bool {
        self.unresolved
            .try_with_value(|held| !held.is_empty())
            .unwrap_or(false)
    }

    /// Settle the held writes, now the connection may be back (see
    /// [`settles_by_key`]). Each one's place in the journal becomes the
    /// entry that undoes what it really did; one that still cannot be
    /// told, or whose scene is not active, stays held for the next try.
    /// One settling runs at a time: a call while it runs has it go round
    /// once more instead.
    pub(super) async fn resolve_unresolved(self) {
        let started = self.settling.try_update_value(|settling| match settling {
            Settling::Idle => {
                *settling = Settling::Running;
                true
            }
            Settling::Running | Settling::Again => {
                *settling = Settling::Again;
                false
            }
        });
        if started != Some(true) {
            return;
        }
        loop {
            let held = self
                .unresolved
                .try_with_value(Clone::clone)
                .unwrap_or_default();
            for item in held {
                if let Some(entry) = self.what_it_did(&item).await {
                    self.settle_if_held(&item, entry);
                }
            }
            let again = self.settling.try_update_value(|settling| {
                let again = *settling == Settling::Again;
                *settling = if again {
                    Settling::Running
                } else {
                    Settling::Idle
                };
                again
            });
            if again != Some(true) {
                return;
            }
        }
    }

    /// Settle `item` with `entry`, while it is still held: a scene switch
    /// meanwhile dropped every held write of the old scene, along with the
    /// journal that held their places. Reports whether it settled.
    pub(super) fn settle_if_held(self, item: &Unresolved, entry: Option<StudioEdit>) -> bool {
        let held = self
            .unresolved
            .try_update_value(|unresolved| {
                let index = unresolved.iter().position(|held| held.token == item.token);
                index.map(|index| unresolved.remove(index)).is_some()
            })
            .unwrap_or(false);
        if held {
            self.settle_held(item, entry);
        }
        held
    }

    /// The entry that undoes what a held write did (`None` for nothing),
    /// or `None` while that cannot be told yet.
    async fn what_it_did(self, item: &Unresolved) -> Option<Option<StudioEdit>> {
        let live = live_scene(item.scene_id).await.ok()?;
        let Pending { fence, key, write } = &item.pending;
        match write {
            HeldWrite::Delete { record, release } => {
                let told = told(item.scene_id, fence, live, || {
                    api::zones::delete_zone(&record.zone_id, fence, key)
                })
                .await?;
                let deleted = match told {
                    Told::Answer(outcome) => {
                        matches!(outcome, Ok(api::zones::ZoneOutcome::Applied(())))
                    }
                    // A zone that is gone was deleted, by this delete or another.
                    Told::Read(scene) => !scene
                        .zones
                        .iter()
                        .any(|zone| zone.id.to_string() == record.zone_id),
                };
                Some(resolve_deletion(deleted, record, release))
            }
            HeldWrite::Members { request, through } => {
                let told = told(item.scene_id, fence, live, || {
                    api::zones::edit_members(request, fence, key)
                })
                .await?;
                match told {
                    // The receipt, possibly a replay of the first answer. The
                    // write placed its outputs, so layout edits made before it
                    // retire; edits made since are newer and stand.
                    Told::Answer(Ok(api::zones::ZoneOutcome::Applied(response))) => {
                        let changes = response.changes;
                        retire_placed(item.scene_id, &changes, *through);
                        self.adopt(response.document);
                        Some((!changes.is_empty()).then_some(StudioEdit::Members(changes)))
                    }
                    Told::Answer(_) => Some(None),
                    // With no receipt to give, the request's own changes stand
                    // in for one. An undo of them reads each output where it
                    // sits now (see `prepare_members_replay`) rather than
                    // trusting these snapshots, so only an output that left the
                    // zone since can stop it, as with any undo.
                    Told::Read(scene) => {
                        if members_landed(&scene, &request.changes) {
                            retire_placed(item.scene_id, &request.changes, *through);
                            return Some(Some(StudioEdit::Members(request.changes.clone())));
                        }
                        if request.changes.is_empty() {
                            toasts::toast_info(
                                "An assignment may have been saved. Check the scene; Undo cannot take it back.",
                            );
                        }
                        Some(None)
                    }
                }
            }
            HeldWrite::Create { name, color } => {
                let told = told(item.scene_id, fence, live, || {
                    api::zones::create_zone(name, color.as_deref(), fence, key)
                })
                .await?;
                match told {
                    Told::Answer(Ok(api::zones::ZoneOutcome::Applied(zone))) => {
                        Some(Some(StudioEdit::ZoneCreated {
                            zone_id: zone.id.to_string(),
                            name: zone.name,
                            color: zone.color,
                            filled: Vec::new(),
                        }))
                    }
                    Told::Answer(_) => Some(None),
                    // Zones may share a name, so no read can tell which one
                    // this request made, if any.
                    Told::Read(_) => {
                        toasts::toast_info(&format!(
                            "{name} may have been made. Check the scene; Undo cannot take it back."
                        ));
                        Some(None)
                    }
                }
            }
            HeldWrite::Patch {
                zone_id,
                before,
                after,
            } => {
                let told = told(item.scene_id, fence, live, || {
                    api::zones::update_zone_keyed(zone_id, after, fence, key)
                })
                .await?;
                let landed = match told {
                    Told::Answer(outcome) => {
                        matches!(outcome, Ok(api::zones::ZoneOutcome::Applied(_)))
                    }
                    Told::Read(scene) => scene
                        .zones
                        .iter()
                        .enumerate()
                        .find(|(_, zone)| zone.id.to_string() == *zone_id)
                        .is_some_and(|(index, zone)| {
                            zone_patch_before(zone, index, after) == *after
                        }),
                };
                Some(landed.then(|| StudioEdit::ZonePatched {
                    zone_id: zone_id.clone(),
                    before: before.clone(),
                    after: after.clone(),
                }))
            }
        }
    }

    /// Put the entry that undoes what a held write did in its place, or
    /// drop the place when it did nothing.
    pub(super) fn settle_held(self, item: &Unresolved, entry: Option<StudioEdit>) {
        self.journal
            .update(|journal| journal.settle_unresolved(item.token, entry));
    }
}
