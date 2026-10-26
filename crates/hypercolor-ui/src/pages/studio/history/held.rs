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
//! commits nothing more, a read of the scene says what it can.

use hypercolor_types::api::scene::{EditMembersRequest, MemberEdit, PatchZoneRequest};
use hypercolor_types::scene::SceneId;
use leptos::prelude::*;

use super::zones::{DeletedZone, live_scene, zone_patch_before};
use super::{Fence, StudioEdit, StudioHistory};
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
    /// A membership edit; its receipt is what the journal keeps.
    Members(EditMembersRequest),
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

/// Whether `scene` holds what a membership edit's `changes` make: every
/// output in the zone the change takes it to, or out of the zone it left.
/// An empty list (an assignment, whose outputs only the daemon names)
/// tells nothing.
#[must_use]
pub fn members_landed(scene: &api::SceneDocument, changes: &[MemberEdit]) -> bool {
    let holds = |zone_id, output_id: &str| {
        scene.zones.iter().any(|zone| {
            zone.id == zone_id && zone.members.iter().any(|member| member.id.0 == output_id)
        })
    };
    !changes.is_empty()
        && changes
            .iter()
            .all(|change| match (&change.after, &change.before) {
                (Some(after), _) => holds(after.zone_id, &after.output.id),
                (None, Some(before)) => !holds(before.zone_id, &before.output.id),
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
        let own_run = settles_by_key(&fence.run, &live.run_epoch);
        match write {
            HeldWrite::Delete { record, release } => {
                let gone = !live
                    .zones
                    .iter()
                    .any(|zone| zone.id.to_string() == record.zone_id);
                let deleted = if own_run {
                    match keyed::send(&fence.run, || {
                        api::zones::delete_zone(&record.zone_id, fence.revision, key)
                    })
                    .await
                    {
                        Answer::Final {
                            outcome: Ok(api::zones::ZoneOutcome::Applied(())),
                            ..
                        } => true,
                        // Not applied now: a zone that is gone anyway was
                        // deleted all the same (a key the daemon no longer
                        // keeps runs afresh, and finds the scene moved on).
                        Answer::Final { .. } => gone,
                        Answer::OtherRun | Answer::Lost(_) => return None,
                    }
                } else {
                    gone
                };
                Some(resolve_deletion(deleted, record, release))
            }
            HeldWrite::Members(request) => {
                if !own_run {
                    if members_landed(&live, &request.changes) {
                        return Some(Some(StudioEdit::Members(request.changes.clone())));
                    }
                    if request.changes.is_empty() {
                        toasts::toast_info(
                            "An assignment may have been saved before the daemon restarted. Check the scene; Undo cannot take it back.",
                        );
                    }
                    return Some(None);
                }
                match keyed::send(&fence.run, || {
                    api::zones::edit_members(request, fence.revision, key)
                })
                .await
                {
                    // The receipt, possibly a replay of the first answer:
                    // layout edits made since stand, so nothing is forgotten.
                    Answer::Final {
                        outcome: Ok(api::zones::ZoneOutcome::Applied(response)),
                        ..
                    } => {
                        let changes = response.changes;
                        self.adopt(response.document);
                        Some((!changes.is_empty()).then_some(StudioEdit::Members(changes)))
                    }
                    Answer::Final { .. } => Some(None),
                    Answer::OtherRun | Answer::Lost(_) => None,
                }
            }
            HeldWrite::Create { name, color } => {
                if !own_run {
                    toasts::toast_info(&format!(
                        "{name} may have been created before the daemon restarted. Check the scene; Undo cannot take it back."
                    ));
                    return Some(None);
                }
                match keyed::send(&fence.run, || {
                    api::zones::create_zone(name, color.as_deref(), fence.revision, key)
                })
                .await
                {
                    Answer::Final {
                        outcome: Ok(api::zones::ZoneOutcome::Applied(zone)),
                        ..
                    } => Some(Some(StudioEdit::ZoneCreated {
                        zone_id: zone.id.to_string(),
                        name: zone.name,
                        color: zone.color,
                        filled: Vec::new(),
                    })),
                    Answer::Final { .. } => Some(None),
                    Answer::OtherRun | Answer::Lost(_) => None,
                }
            }
            HeldWrite::Patch {
                zone_id,
                before,
                after,
            } => {
                let patched = || StudioEdit::ZonePatched {
                    zone_id: zone_id.clone(),
                    before: before.clone(),
                    after: after.clone(),
                };
                if !own_run {
                    let landed = live
                        .zones
                        .iter()
                        .enumerate()
                        .find(|(_, zone)| zone.id.to_string() == *zone_id)
                        .is_some_and(|(index, zone)| {
                            zone_patch_before(zone, index, after) == *after
                        });
                    return Some(landed.then(patched));
                }
                match keyed::send(&fence.run, || {
                    api::zones::update_zone_keyed(zone_id, after, fence.revision, key)
                })
                .await
                {
                    Answer::Final {
                        outcome: Ok(api::zones::ZoneOutcome::Applied(_)),
                        ..
                    } => Some(Some(patched())),
                    Answer::Final { .. } => Some(None),
                    Answer::OtherRun | Answer::Lost(_) => None,
                }
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
