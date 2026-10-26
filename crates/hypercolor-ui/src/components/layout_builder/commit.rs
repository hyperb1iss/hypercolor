//! Live commit of zone layout edits.
//!
//! Every layout edit commits when its gesture ends. Writes are fenced by
//! the scene revision, so they go out one at a time; edits that land while
//! a write is in flight wait in a [`CommitQueue`] that keeps only the
//! newest state per zone. A commit writes what the editor changed against
//! the zone it last loaded, field by field, over the zone as it stands
//! when the write goes out, so a concurrent edit to another output or
//! another field is never put back.

use hypercolor_types::spatial::Output;

use crate::pages::studio::placement::Footprint;

/// A zone's edit waiting to be written: the zone as the editor last loaded
/// it, and the editor's outputs now.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingCommit {
    pub zone_id: String,
    pub baseline: Vec<Output>,
    pub edited: Vec<Output>,
}

/// Zone edits waiting to be written, newest state per zone, in the order
/// the zones were first queued.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommitQueue {
    pending: Vec<PendingCommit>,
}

impl CommitQueue {
    /// Queue `edited` for `zone_id`. A zone already waiting keeps its
    /// older baseline, so the write still carries every edit since.
    pub fn push(&mut self, zone_id: String, baseline: Vec<Output>, edited: Vec<Output>) {
        if let Some(waiting) = self
            .pending
            .iter_mut()
            .find(|waiting| waiting.zone_id == zone_id)
        {
            waiting.edited = edited;
        } else {
            self.pending.push(PendingCommit {
                zone_id,
                baseline,
                edited,
            });
        }
    }

    /// Carry a write that just landed into the edits still waiting for its
    /// zone. Their baseline was the zone before that write; it becomes the
    /// zone after it, so a wait that undoes the write (an output moved and
    /// moved back) still differs from it and is written.
    pub fn landed(&mut self, written: &PendingCommit) {
        for waiting in self
            .pending
            .iter_mut()
            .filter(|waiting| waiting.zone_id == written.zone_id)
        {
            rebase_onto(&mut waiting.baseline, &written.baseline, &written.edited);
        }
    }

    /// Take the next zone to write.
    pub fn pop(&mut self) -> Option<PendingCommit> {
        (!self.pending.is_empty()).then(|| self.pending.remove(0))
    }

    /// Whether a newer state for `zone_id` is still waiting.
    #[must_use]
    pub fn holds(&self, zone_id: &str) -> bool {
        self.pending
            .iter()
            .any(|waiting| waiting.zone_id == zone_id)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// Move one field from `from` to `to` when it changed. A `guarded` move
/// also leaves a field that `target` holds at a value of its own.
fn carry<T: PartialEq + Copy>(target: &mut T, from: T, to: T, guarded: bool) {
    if to != from && (!guarded || *target == from) {
        *target = to;
    }
}

/// Carry the placement fields that changed from `baseline` to `edited`
/// onto `target`. A `guarded` carry skips a field `target` itself changed
/// from `baseline`.
fn carry_placement(target: &mut Output, baseline: &Output, edited: &Output, guarded: bool) {
    carry(
        &mut target.position.x,
        baseline.position.x,
        edited.position.x,
        guarded,
    );
    carry(
        &mut target.position.y,
        baseline.position.y,
        edited.position.y,
        guarded,
    );
    carry(&mut target.size.x, baseline.size.x, edited.size.x, guarded);
    carry(&mut target.size.y, baseline.size.y, edited.size.y, guarded);
    carry(
        &mut target.rotation,
        baseline.rotation,
        edited.rotation,
        guarded,
    );
    carry(&mut target.scale, baseline.scale, edited.scale, guarded);
    carry(
        &mut target.orientation,
        baseline.orientation,
        edited.orientation,
        guarded,
    );
}

/// Carry the placement fields `edited` changed from `baseline` onto
/// `target`, leaving every other field as `target` has it.
fn apply_edit(target: &mut Output, baseline: &Output, edited: &Output) {
    carry_placement(target, baseline, edited, false);
}

fn rebase(outputs: &mut [Output], before: &[Output], after: &[Output], guarded: bool) {
    for output in outputs {
        if let Some(from) = before.iter().find(|from| from.id == output.id)
            && let Some(to) = after.iter().find(|to| to.id == output.id)
        {
            carry_placement(output, from, to, guarded);
        }
    }
}

/// Carry the placement fields that changed from `before` to `after` onto
/// `outputs`, matched by id: a snapshot taken before that change then
/// reads as if it had come after it.
pub fn rebase_onto(outputs: &mut [Output], before: &[Output], after: &[Output]) {
    rebase(outputs, before, after, false);
}

/// Like [`rebase_onto`], for a snapshot with edits of its own: a field the
/// snapshot already changed from `before` keeps its value, and only what
/// it still holds as `before` had it moves to `after`.
pub fn rebase_unedited(outputs: &mut [Output], before: &[Output], after: &[Output]) {
    rebase(outputs, before, after, true);
}

/// Take one field to `after` when `seen` already shows that write landed.
fn recognize<T: PartialEq + Copy>(target: &mut T, before: T, after: T, seen: T) {
    if after != before && seen == after {
        *target = after;
    }
}

/// Recognize a write still waiting for its reply in a refresh that already
/// carries it. On each field `written` changed and `canonical` now holds at
/// the written value, `baseline` takes that value, so the editor's edits
/// since read as edits from what landed rather than snapping back to it. A
/// refresh that does not carry the write leaves `baseline` alone.
pub fn recognize_write(baseline: &mut [Output], canonical: &[Output], written: &PendingCommit) {
    for output in baseline {
        let find = |outputs: &[Output]| outputs.iter().find(|other| other.id == output.id).cloned();
        let (Some(before), Some(after), Some(seen)) = (
            find(&written.baseline),
            find(&written.edited),
            find(canonical),
        ) else {
            continue;
        };
        recognize(
            &mut output.position.x,
            before.position.x,
            after.position.x,
            seen.position.x,
        );
        recognize(
            &mut output.position.y,
            before.position.y,
            after.position.y,
            seen.position.y,
        );
        recognize(&mut output.size.x, before.size.x, after.size.x, seen.size.x);
        recognize(&mut output.size.y, before.size.y, after.size.y, seen.size.y);
        recognize(
            &mut output.rotation,
            before.rotation,
            after.rotation,
            seen.rotation,
        );
        recognize(&mut output.scale, before.scale, after.scale, seen.scale);
        recognize(
            &mut output.orientation,
            before.orientation,
            after.orientation,
            seen.orientation,
        );
    }
}

/// The editor's outputs once a canvas gesture ends: `current` as the
/// editor holds them now, each carrying only the placement fields the
/// gesture changed from `base`, the outputs at its press. A change that
/// reached the editor from elsewhere while the gesture ran stays, so a
/// release never puts back another client's move.
#[must_use]
pub fn merge_gesture(current: &[Output], base: &[Output], gestured: &[Output]) -> Vec<Output> {
    current
        .iter()
        .map(|output| {
            let mut merged = output.clone();
            if let Some(before) = base.iter().find(|before| before.id == output.id)
                && let Some(after) = gestured.iter().find(|after| after.id == output.id)
            {
                apply_edit(&mut merged, before, after);
            }
            merged
        })
        .collect()
}

/// The outputs a zone commit writes: the zone's current `members`, in
/// their order, each carrying the placement fields the editor changed
/// from `baseline`. Outputs match by member id, and only placement fields
/// carry over, so a commit can never add, drop, or rebind a member.
/// `None` when the result would not change the zone.
#[must_use]
pub fn commit_placements(
    members: &[Output],
    baseline: &[Output],
    edited: &[Output],
) -> Option<Vec<Output>> {
    let same = |a: &Output, b: &Output| a.id == b.id;
    let mut changed = false;
    let placements = members
        .iter()
        .map(|member| {
            let mut placed = member.clone();
            if let Some(local) = edited.iter().find(|local| same(local, member))
                && let Some(loaded) = baseline.iter().find(|loaded| same(loaded, member))
            {
                apply_edit(&mut placed, loaded, local);
                changed |= Footprint::of(&placed) != Footprint::of(member);
            }
            placed
        })
        .collect();
    changed.then_some(placements)
}
