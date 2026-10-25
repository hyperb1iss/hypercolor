//! Live commit of zone layout edits.
//!
//! Every layout edit commits when its gesture ends. Writes are fenced by
//! the scene revision, so they go out one at a time; edits that land while
//! a write is in flight wait in a [`CommitQueue`] that keeps only the
//! newest state per zone, since each write carries the zone's complete
//! placement set.

use hypercolor_types::spatial::Output;

use crate::pages::studio::placement::Footprint;

/// Zone layouts waiting to be written: the newest edited outputs per zone,
/// in the order the zones were first queued.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommitQueue {
    pending: Vec<(String, Vec<Output>)>,
}

impl CommitQueue {
    /// Queue `outputs` as the state to write for `zone_id`, replacing any
    /// state still waiting for that zone.
    pub fn push(&mut self, zone_id: String, outputs: Vec<Output>) {
        if let Some((_, waiting)) = self.pending.iter_mut().find(|(id, _)| *id == zone_id) {
            *waiting = outputs;
        } else {
            self.pending.push((zone_id, outputs));
        }
    }

    /// Take the next zone to write.
    pub fn pop(&mut self) -> Option<(String, Vec<Output>)> {
        (!self.pending.is_empty()).then(|| self.pending.remove(0))
    }

    /// Whether a newer state for `zone_id` is still waiting.
    #[must_use]
    pub fn holds(&self, zone_id: &str) -> bool {
        self.pending.iter().any(|(id, _)| id == zone_id)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// The outputs a zone commit writes: the zone's current members, in their
/// order, each carrying the edited placement when the editor holds the
/// same output with the same binding. Members the editor does not hold
/// keep their placement, so a commit can never add, drop, or rebind one.
/// `None` when the result would not change the zone.
#[must_use]
pub fn commit_placements(members: &[Output], edited: &[Output]) -> Option<Vec<Output>> {
    let mut changed = false;
    let placements = members
        .iter()
        .map(|member| {
            let mut placed = member.clone();
            if let Some(local) = edited.iter().find(|local| {
                local.id == member.id
                    && local.device_id == member.device_id
                    && local.zone_name == member.zone_name
            }) {
                let footprint = Footprint::of(local);
                if footprint != Footprint::of(member) {
                    footprint.apply(&mut placed);
                    changed = true;
                }
            }
            placed
        })
        .collect();
    changed.then_some(placements)
}
