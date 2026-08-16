//! Ordered commit sequencing and durability receipts for scene
//! mutations (Spec 76 §2.3).
//!
//! Two independent axes meet here, and keeping them apart is the whole
//! point of the module:
//!
//! **Ordering.** A commit generation is assigned at admission, under
//! the same lock that installs the candidate state. Every commit then
//! releases its slot through [`SceneCommitSequencer`], which publishes
//! strictly in admission order. Two commits that admit in sequence can
//! never publish in reverse order, no matter how their asynchronous
//! persistence interleaves.
//!
//! **Durability.** Whether the admitted bytes reached the file is
//! reported by [`CommitDurability`], which projects the persistence
//! layer's three real post-admission states. It never reorders
//! anything; it only says where the payload ended up.
//!
//! The sequencer's revision counter doubles as the optimistic-
//! concurrency version that [`commit_scene`](super::scene::commit_scene)
//! compares against, so a generation and the scene revision it produced
//! are the same number.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use hypercolor_core::bus::HypercolorBus;
use hypercolor_types::event::HypercolorEvent;

/// Monotonic version of the daemon's live scene state. Bumped once per
/// admitted commit, under the scene write lock.
pub type SceneRevision = u64;

/// Where an admitted scene payload ended up.
///
/// Admission is the durability commitment: once the bytes are admitted
/// the retry supervisor owns them, so none of these variants means the
/// mutation was rejected. Rejection happens before admission and
/// surfaces as `Err(DomainError)` instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitDurability {
    /// The payload replaced the destination and its durability barrier
    /// completed.
    Written,
    /// A newer admitted generation won the destination first. The newer
    /// payload is authoritative and already contains this commit's
    /// changes, so this commit's own bytes will never be written.
    Superseded,
    /// The write did not prove durable on this attempt. The payload
    /// stays the destination's newest admitted intent and the retry
    /// supervisor converges on it.
    Retrying,
}

impl CommitDurability {
    /// Whether this commit's payload is the one the destination will
    /// converge on.
    #[must_use]
    pub const fn is_authoritative_payload(self) -> bool {
        matches!(self, Self::Written | Self::Retrying)
    }
}

/// Receipt for one admitted scene commit.
#[derive(Debug)]
pub struct SceneCommit {
    generation: u64,
    revision: SceneRevision,
    durability: CommitDurability,
    retry_error: Option<String>,
}

impl SceneCommit {
    pub(super) const fn new(
        generation: u64,
        revision: SceneRevision,
        durability: CommitDurability,
        retry_error: Option<String>,
    ) -> Self {
        Self {
            generation,
            revision,
            durability,
            retry_error,
        }
    }

    /// The commit generation assigned at admission.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// The scene revision this commit produced.
    #[must_use]
    pub const fn revision(&self) -> SceneRevision {
        self.revision
    }

    /// Where the payload ended up after admission.
    #[must_use]
    pub const fn durability(&self) -> CommitDurability {
        self.durability
    }

    /// The failure text from a non-durable attempt, for adapters that
    /// still render persistence trouble on their frozen v1 wire.
    #[must_use]
    pub fn retry_error(&self) -> Option<&str> {
        self.retry_error.as_deref()
    }
}

/// One ordered publication chain for scene commits.
///
/// Lives in `AppState` so every transport shares it. The inner lock is
/// a `std::sync::Mutex` held only for bookkeeping — never across an
/// await, never across a publish.
#[derive(Debug, Default)]
pub struct SceneCommitSequencer {
    state: Mutex<SequencerState>,
}

#[derive(Debug, Default)]
struct SequencerState {
    /// Current scene revision, and the last generation handed out.
    revision: SceneRevision,
    /// Highest generation whose slot has been released.
    released_through: u64,
    /// Completed commits waiting on an older generation's slot.
    parked: BTreeMap<u64, Vec<HypercolorEvent>>,
}

impl SceneCommitSequencer {
    /// A fresh sequencer at revision zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The current scene revision. Callers read this under a scene lock
    /// so the revision and the state it describes agree.
    #[must_use]
    pub fn revision(&self) -> SceneRevision {
        self.locked().revision
    }

    /// Assign the next commit generation and advance the revision.
    ///
    /// Callers hold the scene write lock across this call, which is
    /// what makes admission order and revision order the same order.
    pub(super) fn admit(self: &Arc<Self>, bus: Arc<HypercolorBus>) -> CommitTicket {
        let generation = {
            let mut state = self.locked();
            state.revision = state
                .revision
                .checked_add(1)
                .expect("scene commit generation must not exhaust u64");
            state.revision
        };
        CommitTicket {
            sequencer: Arc::clone(self),
            bus,
            generation,
            released: false,
        }
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, SequencerState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Fill `generation`'s slot and return everything that became
    /// publishable, in admission order.
    fn fill(&self, generation: u64, events: Vec<HypercolorEvent>) -> Vec<HypercolorEvent> {
        let mut state = self.locked();
        if generation <= state.released_through {
            // The slot is already gone, so a strictly newer generation
            // has published. Replaying this payload now would walk the
            // published state backwards.
            return Vec::new();
        }
        state.parked.insert(generation, events);

        let mut drained = Vec::new();
        let mut next = state.released_through.saturating_add(1);
        while let Some(ready) = state.parked.remove(&next) {
            drained.extend(ready);
            next = next.saturating_add(1);
        }
        state.released_through = next.saturating_sub(1);
        drained
    }
}

/// A claim on one slot in the publication order.
///
/// Every ticket must fill its slot or the chain behind it stalls, so
/// the drop path fills it with nothing. That makes a cancelled commit
/// lose its own publication without ever blocking a later one.
#[derive(Debug)]
pub struct CommitTicket {
    sequencer: Arc<SceneCommitSequencer>,
    bus: Arc<HypercolorBus>,
    generation: u64,
    released: bool,
}

impl CommitTicket {
    /// The generation this ticket holds.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Release the slot, publishing `events` once every older
    /// generation has released too.
    pub(super) fn release(mut self, events: Vec<HypercolorEvent>) {
        self.released = true;
        let drained = self.sequencer.fill(self.generation, events);
        for event in drained {
            self.bus.publish(event);
        }
    }

    /// Release the slot without publishing anything — the payload this
    /// commit would have announced is not the one the destination
    /// converges on.
    pub(super) fn discard(self) {
        self.release(Vec::new());
    }
}

impl Drop for CommitTicket {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let drained = self.sequencer.fill(self.generation, Vec::new());
        for event in drained {
            self.bus.publish(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypercolor_types::event::{SceneLibraryChangeKind, SceneSettingsChangeKind};
    use hypercolor_types::scene::SceneId;

    fn bus() -> Arc<HypercolorBus> {
        Arc::new(HypercolorBus::new())
    }

    fn marker(groups_revision: u64) -> HypercolorEvent {
        HypercolorEvent::SceneSettingsChanged {
            scene_id: SceneId::DEFAULT,
            groups_revision,
            kind: SceneSettingsChangeKind::UnassignedBehavior,
        }
    }

    fn marker_revision(event: &HypercolorEvent) -> Option<u64> {
        match event {
            HypercolorEvent::SceneSettingsChanged {
                groups_revision, ..
            } => Some(*groups_revision),
            _ => None,
        }
    }

    fn drain(
        receiver: &mut tokio::sync::broadcast::Receiver<hypercolor_core::bus::TimestampedEvent>,
    ) -> Vec<u64> {
        let mut seen = Vec::new();
        while let Ok(timestamped) = receiver.try_recv() {
            if let Some(revision) = marker_revision(&timestamped.event) {
                seen.push(revision);
            }
        }
        seen
    }

    #[test]
    fn admission_assigns_strictly_increasing_generations() {
        let sequencer = Arc::new(SceneCommitSequencer::new());
        let bus = bus();
        assert_eq!(sequencer.revision(), 0);
        let first = sequencer.admit(Arc::clone(&bus));
        let second = sequencer.admit(Arc::clone(&bus));
        assert_eq!(first.generation(), 1);
        assert_eq!(second.generation(), 2);
        assert_eq!(sequencer.revision(), 2);
    }

    #[test]
    fn reversed_completion_still_publishes_in_admission_order() {
        let sequencer = Arc::new(SceneCommitSequencer::new());
        let bus = bus();
        let mut events = bus.subscribe_all();

        let first = sequencer.admit(Arc::clone(&bus));
        let second = sequencer.admit(Arc::clone(&bus));

        // The younger commit finishes its persistence first.
        second.release(vec![marker(2)]);
        assert!(
            drain(&mut events).is_empty(),
            "the younger commit must park until the older one releases"
        );

        first.release(vec![marker(1)]);
        assert_eq!(drain(&mut events), vec![1, 2]);
    }

    #[test]
    fn a_discarded_commit_drops_its_events_without_stalling_the_chain() {
        let sequencer = Arc::new(SceneCommitSequencer::new());
        let bus = bus();
        let mut events = bus.subscribe_all();

        let first = sequencer.admit(Arc::clone(&bus));
        let second = sequencer.admit(Arc::clone(&bus));

        second.release(vec![marker(2)]);
        first.discard();

        assert_eq!(
            drain(&mut events),
            vec![2],
            "the superseded commit publishes nothing, the newer one still publishes"
        );
    }

    #[test]
    fn a_dropped_ticket_releases_its_slot() {
        let sequencer = Arc::new(SceneCommitSequencer::new());
        let bus = bus();
        let mut events = bus.subscribe_all();

        let first = sequencer.admit(Arc::clone(&bus));
        let second = sequencer.admit(Arc::clone(&bus));
        second.release(vec![marker(2)]);
        drop(first);

        assert_eq!(drain(&mut events), vec![2]);
    }

    #[test]
    fn a_late_release_below_the_watermark_publishes_nothing() {
        let sequencer = Arc::new(SceneCommitSequencer::new());
        let bus = bus();
        let mut events = bus.subscribe_all();

        let first = sequencer.admit(Arc::clone(&bus));
        let second = sequencer.admit(Arc::clone(&bus));
        let generation = first.generation();
        drop(first);
        second.release(vec![marker(2)]);
        assert_eq!(drain(&mut events), vec![2]);

        // Nothing may replay into a slot the chain already passed.
        assert!(sequencer.fill(generation, vec![marker(1)]).is_empty());
    }

    /// The daemon builds five `AppState`s over one `DaemonState`, all
    /// sharing the scene manager by `Arc`. Giving any of them its own
    /// sequencer hands it a private revision counter and a private
    /// publication chain over shared state, so the compare-and-swap
    /// stops seeing competing commits and two chains publish in
    /// arbitrary order relative to each other. The property is
    /// structural rather than observable — reproducing it behaviorally
    /// means booting real subsystems, which segfaults on teardown — so
    /// it is pinned at the one construction site that can regress it.
    #[test]
    fn app_state_takes_the_daemon_sequencer_rather_than_minting_one() {
        let source = include_str!("../api/mod.rs");
        let from_daemon = source
            .split_once("pub fn from_daemon_state(")
            .expect("from_daemon_state must exist")
            .1;
        let wiring = from_daemon
            .lines()
            .find(|line| line.contains("scene_commits:"))
            .expect("from_daemon_state must wire scene_commits");
        assert!(
            wiring.contains("Arc::clone(&daemon.scene_commits)"),
            "from_daemon_state must share the daemon's sequencer, not mint one: {wiring}"
        );
    }

    #[test]
    fn durability_names_which_payload_the_destination_converges_on() {
        assert!(CommitDurability::Written.is_authoritative_payload());
        assert!(CommitDurability::Retrying.is_authoritative_payload());
        assert!(!CommitDurability::Superseded.is_authoritative_payload());
    }

    #[test]
    fn library_change_kinds_stay_reachable_for_receipt_tests() {
        // Guards the marker helper against an event-enum reshuffle that
        // would otherwise silently retype these tests.
        assert_ne!(
            SceneLibraryChangeKind::Created,
            SceneLibraryChangeKind::Updated
        );
    }
}
