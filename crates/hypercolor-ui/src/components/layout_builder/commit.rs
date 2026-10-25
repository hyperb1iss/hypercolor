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

/// Carry the placement fields `edited` changed from `baseline` onto
/// `target`, leaving every other field as `target` has it.
fn apply_edit(target: &mut Output, baseline: &Output, edited: &Output) {
    if edited.position.x != baseline.position.x {
        target.position.x = edited.position.x;
    }
    if edited.position.y != baseline.position.y {
        target.position.y = edited.position.y;
    }
    if edited.size.x != baseline.size.x {
        target.size.x = edited.size.x;
    }
    if edited.size.y != baseline.size.y {
        target.size.y = edited.size.y;
    }
    if edited.rotation != baseline.rotation {
        target.rotation = edited.rotation;
    }
    if edited.scale != baseline.scale {
        target.scale = edited.scale;
    }
    if edited.orientation != baseline.orientation {
        target.orientation = edited.orientation;
    }
}

/// The outputs a zone commit writes: the zone's current `members`, in
/// their order, each carrying the placement fields the editor changed
/// from `baseline`. Only an output with the same id and binding in all
/// three takes an edit, so a commit can never add, drop, or rebind a
/// member. `None` when the result would not change the zone.
#[must_use]
pub fn commit_placements(
    members: &[Output],
    baseline: &[Output],
    edited: &[Output],
) -> Option<Vec<Output>> {
    let same = |a: &Output, b: &Output| {
        a.id == b.id && a.device_id == b.device_id && a.zone_name == b.zone_name
    };
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
