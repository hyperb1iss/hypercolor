//! History's writes whose outcome a lost connection hid.
//!
//! Every write of Studio's history goes out under an idempotency key and
//! goes out again under that key while no answer comes back (see
//! [`crate::components::layout_builder::keyed`]). When even that gets no
//! answer, or another daemon run answers, the write may have landed or
//! not. It then holds its place in the journal
//! ([`StudioEdit::Unresolved`]) with everything needed to settle it, and
//! neither undo nor redo moves past it until the connection is back. An
//! undo or redo is held the same way: its place keeps the step it
//! replays, so whatever its write did, the journal ends up saying so, and
//! a multi-step one goes on from where it was once its write is known to
//! have landed (see [`Then`]). In
//! its own daemon run it goes out again under its key, and the daemon's
//! answer says what it did (a replay of the first answer when the first
//! request landed, receipt and all). Once that run has ended, which
//! commits nothing more, or the daemon has let the first answer go, which
//! it does only once that request is done, a read of the scene says what
//! it can.

use hypercolor_types::api::scene::{EditMembersRequest, MemberEdit, PatchZoneRequest};
use hypercolor_types::scene::{SceneId, ZoneId};
use leptos::prelude::*;

use super::super::placement::Footprint;
use super::zones::{DeletedZone, live_scene, zone_patch_before};
use super::{EntryId, Fence, Settled, StudioEdit, StudioHistory, flip, retire_placed};
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
    /// A zone create, with the zones the scene held as it went out, so a
    /// read can tell the zone it made from the others of its name.
    Create {
        name: String,
        color: Option<String>,
        before: Vec<ZoneId>,
    },
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

/// The rest of an undo or redo of zone structure whose first write was
/// held: it runs once that write is known to have landed.
#[derive(Clone, Debug)]
pub(super) enum Resume {
    /// Undoing a delete: the zone was made again, and its devices, layers,
    /// and settings follow. `remove` drops the place when done, for a
    /// restore from the Undo bar.
    Rebuild { record: DeletedZone, remove: bool },
    /// Undoing a zone's creation: its devices went back, and the zone goes
    /// if nothing else is in it.
    Uncreate {
        zone_id: String,
        neighbor: Option<String>,
        name: String,
        color: Option<String>,
        filled: Vec<MemberEdit>,
    },
    /// Redoing a zone's creation: the zone was made again, under a new id,
    /// and its devices follow.
    Recreate {
        zone_id: String,
        name: String,
        color: Option<String>,
        filled: Vec<MemberEdit>,
    },
}

impl Resume {
    /// What the place holds when the first write did not land: the step
    /// as it was.
    fn not_landed(&self) -> Settled {
        match self {
            Self::Rebuild { record, .. } => {
                Settled::Applied(StudioEdit::ZoneDeleted(Box::new(record.clone())))
            }
            Self::Uncreate {
                zone_id,
                name,
                color,
                filled,
                ..
            } => Settled::Applied(StudioEdit::ZoneCreated {
                zone_id: zone_id.clone(),
                name: name.clone(),
                color: color.clone(),
                filled: filled.clone(),
            }),
            Self::Recreate {
                zone_id,
                name,
                color,
                filled,
            } => Settled::Unapplied(StudioEdit::ZoneCreated {
                zone_id: zone_id.clone(),
                name: name.clone(),
                color: color.clone(),
                filled: filled.clone(),
            }),
        }
    }
}

/// What a held write's place in the journal becomes once it settles.
#[derive(Clone, Debug)]
pub(super) enum Then {
    /// A new edit: what it did becomes its entry, and nothing leaves none.
    Record,
    /// An undo (`redo` false) or redo of `entry`, all in this one write.
    /// Landed, the place holds the step, applied for a redo and not for an
    /// undo; otherwise it holds `entry` as it was.
    Replay { entry: StudioEdit, redo: bool },
    /// A step whose landing alone decides its place.
    Either { landed: Settled, not: Settled },
    /// The move into a zone a redo made again: the place becomes that
    /// zone's creation, with what the move moved.
    Fill {
        zone_id: String,
        name: String,
        color: Option<String>,
    },
    /// The first write of a multi-step undo or redo.
    Resume(Box<Resume>),
}

/// A held write and its place in the journal.
#[derive(Clone, Debug)]
pub(super) struct Unresolved {
    pub scene_id: SceneId,
    /// Its place in the journal.
    pub entry: EntryId,
    /// What it was, as the user hears of it.
    pub name: String,
    pub pending: Pending,
    pub then: Then,
    /// What it did, once known, for an undo or redo whose rest has to wait
    /// for the next settling: it is not asked again.
    pub learned: Option<StudioEdit>,
}

/// The step an undo's landed write leaves on the redo side: a membership
/// undo keeps its receipt turned back into the move it undid; any other
/// keeps the entry it undid.
fn undone(entry: &StudioEdit, did: StudioEdit) -> StudioEdit {
    match did {
        StudioEdit::Members(changes) => {
            StudioEdit::Members(changes.into_iter().map(flip).collect())
        }
        _ => entry.clone(),
    }
}

/// Tell the user a write, which `what` names, is held.
fn tell_held(what: &str) {
    toasts::toast_info(&format!(
        "No answer came back for {what}, so it may or may not have been saved. It settles once the connection is back, and Undo and Redo wait for it."
    ));
}

/// Say that a held write did not go through, `name` saying what it was.
fn did_not_land(name: &str) {
    let mut chars = name.chars();
    let name = chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    });
    toasts::toast_info(&format!("{name} did not go through."));
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

/// Which zone a create named `name` made, as a read of the scene tells.
#[derive(Debug)]
pub enum Made<'a> {
    /// No zone of that name is new since the create went out.
    None,
    /// The one new zone of that name.
    One(&'a api::ZoneResource),
    /// More than one is, so the read cannot tell which.
    Several,
}

/// The zone a create named `name` made, judged from `scene` and the zones
/// the scene held as the create went out (`before`): the zone of that
/// name that is new since.
#[must_use]
pub fn zone_it_made<'a>(scene: &'a api::SceneDocument, name: &str, before: &[ZoneId]) -> Made<'a> {
    let mut new = scene
        .zones
        .iter()
        .filter(|zone| zone.name == name && !before.contains(&zone.id));
    match (new.next(), new.next()) {
        (None, _) => Made::None,
        (Some(zone), None) => Made::One(zone),
        (Some(_), Some(_)) => Made::Several,
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
    /// Hold `pending`, whose outcome is unknown, at the journal place
    /// `entry`, which already shows as unresolved, until it settles into
    /// what `then` says.
    pub(super) fn hold(
        self,
        scene_id: SceneId,
        entry: EntryId,
        name: String,
        pending: Pending,
        then: Then,
    ) {
        self.unresolved.try_update_value(|unresolved| {
            unresolved.push(Unresolved {
                scene_id,
                entry,
                name,
                pending,
                then,
                learned: None,
            });
        });
    }

    /// Record a new journal place for `pending`, which `what` names, hold
    /// it there, and tell the user.
    pub(super) fn record_held(self, scene_id: SceneId, what: String, pending: Pending) {
        tell_held(&what);
        let name = what.clone();
        let entry = self
            .journal
            .try_update(|journal| journal.record(StudioEdit::Unresolved { name }));
        if let Some(entry) = entry {
            self.hold(scene_id, entry, what, pending, Then::Record);
        }
    }

    /// Hold the undo or redo replaying the journal place `entry`, which
    /// `what` names, until `pending` settles into what `then` says, and
    /// tell the user.
    pub(super) fn hold_replay(
        self,
        scene_id: SceneId,
        entry: EntryId,
        what: String,
        pending: Pending,
        then: Then,
    ) {
        tell_held(&what);
        self.hold_place(scene_id, entry, what, pending, then);
    }

    /// [`Self::hold_replay`] for a write the user already heard of.
    pub(super) fn hold_place(
        self,
        scene_id: SceneId,
        entry: EntryId,
        what: String,
        pending: Pending,
        then: Then,
    ) {
        let name = what.clone();
        self.journal.update(|journal| journal.hold(entry, name));
        self.hold(scene_id, entry, what, pending, then);
    }

    /// Hold `item` again, its place still waiting, for the next settling
    /// to go on from what it already learned it `did`.
    pub(super) fn hold_again(self, item: Unresolved, did: StudioEdit) {
        self.unresolved.try_update_value(|unresolved| {
            unresolved.push(Unresolved {
                learned: Some(did),
                ..item
            });
        });
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
                let did = match &item.learned {
                    Some(did) => Some(Some(did.clone())),
                    None => self.what_it_did(&item).await,
                };
                if let Some(did) = did {
                    self.settle_if_held(&item, did);
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

    /// Settle `item` with `did`, the entry that undoes what its write did
    /// (`None` for nothing), while it is still held: a scene switch
    /// meanwhile dropped every held write of the old scene, along with the
    /// journal that held their places. Reports whether it settled.
    pub(super) fn settle_if_held(self, item: &Unresolved, did: Option<StudioEdit>) -> bool {
        let held = self
            .unresolved
            .try_update_value(|unresolved| {
                let index = unresolved.iter().position(|held| held.entry == item.entry);
                index.map(|index| unresolved.remove(index)).is_some()
            })
            .unwrap_or(false);
        if held {
            self.settle_held(item, did);
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
                if !deleted {
                    did_not_land(&item.name);
                }
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
                    Told::Answer(_) => {
                        did_not_land(&item.name);
                        Some(None)
                    }
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
                        } else {
                            did_not_land(&item.name);
                        }
                        Some(None)
                    }
                }
            }
            HeldWrite::Create {
                name,
                color,
                before,
            } => {
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
                    Told::Answer(_) => {
                        did_not_land(&item.name);
                        Some(None)
                    }
                    // Zones may share a name, so only a zone of that name the
                    // scene did not hold as the create went out can be its.
                    Told::Read(scene) => match zone_it_made(&scene, name, before) {
                        Made::One(zone) => Some(Some(StudioEdit::ZoneCreated {
                            zone_id: zone.id.to_string(),
                            name: zone.name.clone(),
                            color: zone.color.clone(),
                            filled: Vec::new(),
                        })),
                        Made::None => {
                            did_not_land(&item.name);
                            Some(None)
                        }
                        Made::Several => {
                            toasts::toast_info(&format!(
                                "{name} may have been made, but more than one new zone has that name. Check the scene."
                            ));
                            Some(None)
                        }
                    },
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
                if !landed {
                    did_not_land(&item.name);
                }
                Some(landed.then(|| StudioEdit::ZonePatched {
                    zone_id: zone_id.clone(),
                    before: before.clone(),
                    after: after.clone(),
                }))
            }
        }
    }

    /// Settle a held write's place with `did`, the entry that undoes what
    /// it did (`None` for nothing), as its [`Then`] says. A multi-step undo
    /// or redo whose first write landed goes on in its turn with the rest,
    /// and its place waits until that is done.
    pub(super) fn settle_held(self, item: &Unresolved, did: Option<StudioEdit>) {
        let settled = match (&item.then, did) {
            (Then::Record, Some(did)) => Settled::Applied(did),
            (Then::Record, None) => Settled::Gone,
            (Then::Replay { redo: true, .. }, Some(did)) => Settled::Applied(did),
            (Then::Replay { entry, redo: true }, None) => Settled::Unapplied(entry.clone()),
            (Then::Replay { entry, redo: false }, Some(did)) => {
                Settled::Unapplied(undone(entry, did))
            }
            (Then::Replay { entry, redo: false }, None) => Settled::Applied(entry.clone()),
            (Then::Either { landed, .. }, Some(_)) => landed.clone(),
            (Then::Either { not, .. }, None) => not.clone(),
            (
                Then::Fill {
                    zone_id,
                    name,
                    color,
                },
                did,
            ) => Settled::Applied(StudioEdit::ZoneCreated {
                zone_id: zone_id.clone(),
                name: name.clone(),
                color: color.clone(),
                filled: match did {
                    Some(StudioEdit::Members(changes)) => changes,
                    _ => Vec::new(),
                },
            }),
            (Then::Resume(_), Some(did)) => {
                self.when_free(super::Waiting::Resume {
                    item: Box::new(item.clone()),
                    did,
                });
                return;
            }
            (Then::Resume(resume), None) => resume.not_landed(),
        };
        self.journal
            .update(|journal| journal.settle(item.entry, settled));
    }
}
