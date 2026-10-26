//! One chronological journal for Studio: layout placements, membership,
//! and zone structure (create, rename and the other zone fields, delete),
//! each undone and redone in order.
//!
//! Every write is fenced by the scene revision, so one operation runs at a
//! time. `busy` marks a membership or zone operation or a layout replay in
//! flight, and locks the canvas; `committing` marks the layout commit queue
//! draining, which leaves editing alone. An operation asked for while
//! either runs waits its turn in order instead of being dropped.

use std::collections::VecDeque;

use super::placement::{self, Footprint};
use crate::components::layout_builder::keyed;
use crate::{api, layout_history::LayoutEditorSnapshot, toasts};
use hypercolor_types::api::scene::{
    EditMembersRequest, EditMembersResponse, MemberEdit, MemberPlacementHint, PatchZoneRequest,
};
use hypercolor_types::scene::{SceneId, UnassignedBehavior};
use hypercolor_types::spatial::NormalizedPosition;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;

/// Runs once a membership write settles, with the scene revision it
/// landed at, or `None` when it did not apply. Every operation that takes
/// one settles it exactly once: when it lands, fails, stands down, or is
/// dropped from the queue. Only a Studio torn down mid-write drops it
/// unsettled, since nothing is left to hear it.
pub(crate) type EditDone = Box<dyn FnOnce(Option<u64>)>;

/// Where a scene write is fenced: a scene revision, and the daemon run it
/// belongs to, which the write's idempotency key is good for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fence {
    pub revision: u64,
    pub run: String,
}

impl Fence {
    /// The fence a write against `scene` as read carries.
    #[must_use]
    pub(crate) fn of(scene: &api::SceneDocument) -> Self {
        Self {
            revision: scene.revision,
            run: scene.run_epoch.clone(),
        }
    }

    /// The same run's fence at `revision`.
    #[must_use]
    pub(crate) fn at(&self, revision: u64) -> Self {
        Self {
            revision,
            run: self.run.clone(),
        }
    }
}

mod announce;
mod zones;

pub use announce::{DeviceLookup, ZoneLookup, replay_announcement};

pub use zones::{
    DeletedZone, light_neighbor, rename_is_noop, resolve_deletion, restore_layer_requests,
    settles_by_key, zone_patch_before,
};
pub(crate) use zones::{Moved, NewZone};

/// The scene Studio's history belongs to: the scene read now, or, while a
/// read has failed and none is known, the one it already belonged to.
#[must_use]
pub fn held_scene(read: Option<SceneId>, held: Option<SceneId>) -> Option<SceneId> {
    read.or(held)
}

#[derive(Clone, Debug, PartialEq)]
pub enum StudioEdit {
    Layout {
        zone_id: String,
        before: Box<LayoutEditorSnapshot>,
        after: Box<LayoutEditorSnapshot>,
    },
    Members(Vec<MemberEdit>),
    /// A zone Studio made, with the move that filled it as it was made.
    ZoneCreated {
        zone_id: String,
        name: String,
        color: Option<String>,
        filled: Vec<MemberEdit>,
    },
    /// A rename, recolor, pause or resume, or reorder: the changed fields
    /// as they were and as they became.
    ZonePatched {
        zone_id: String,
        before: PatchZoneRequest,
        after: PatchZoneRequest,
    },
    ZoneDeleted(Box<DeletedZone>),
    /// A zone delete whose outcome a lost connection hid. It holds its
    /// place in the journal until a scene read says what it did, then
    /// becomes that entry; until then neither undo nor redo moves past it.
    Unresolved {
        token: u64,
        name: String,
    },
}

#[derive(Clone, Debug, Default)]
pub struct StudioJournal {
    entries: Vec<StudioEdit>,
    cursor: usize,
    /// Edits recorded so far; replays do not count.
    recorded: u64,
}

impl StudioJournal {
    pub fn record(&mut self, edit: StudioEdit) {
        self.recorded = self.recorded.wrapping_add(1);
        self.entries.truncate(self.cursor);
        self.entries.push(edit);
        if self.entries.len() > crate::layout_history::MAX_HISTORY_DEPTH {
            self.entries.remove(0);
        }
        self.cursor = self.entries.len();
    }
    pub fn undo_edit(&self) -> Option<&StudioEdit> {
        self.cursor.checked_sub(1).and_then(|i| self.entries.get(i))
    }
    pub fn redo_edit(&self) -> Option<&StudioEdit> {
        self.entries.get(self.cursor)
    }
    /// Changes whenever a new edit is recorded, so a waiting undo or redo
    /// can tell the history moved on after it was asked for.
    pub fn recorded(&self) -> u64 {
        self.recorded
    }
    /// Point every entry that names the zone `old` at `new`, once a
    /// restore or redo has made that zone again under a new id.
    pub fn remap_zone(&mut self, old: &str, new: hypercolor_types::scene::ZoneId) {
        let remap_changes = |changes: &mut Vec<MemberEdit>| {
            for state in changes
                .iter_mut()
                .flat_map(|change| change.before.iter_mut().chain(change.after.iter_mut()))
            {
                if state.zone_id.to_string() == old {
                    state.zone_id = new;
                }
            }
        };
        let new_key = new.to_string();
        for entry in &mut self.entries {
            match entry {
                StudioEdit::Layout { zone_id, .. } | StudioEdit::ZonePatched { zone_id, .. } => {
                    if zone_id == old {
                        zone_id.clone_from(&new_key);
                    }
                }
                StudioEdit::Members(changes) => remap_changes(changes),
                StudioEdit::ZoneCreated {
                    zone_id, filled, ..
                } => {
                    if zone_id == old {
                        zone_id.clone_from(&new_key);
                    }
                    remap_changes(filled);
                }
                StudioEdit::ZoneDeleted(record) => {
                    if record.zone_id == old {
                        record.zone_id.clone_from(&new_key);
                    }
                }
                StudioEdit::Unresolved { .. } => {}
            }
        }
    }

    /// Settle the unresolved delete `token` names: its place becomes
    /// `entry`, or goes when the delete changed nothing. A journal that no
    /// longer holds it (a scene switch cleared it) is left alone.
    pub fn settle_unresolved(&mut self, token: u64, entry: Option<StudioEdit>) {
        let Some(index) = self.entries.iter().position(
            |held| matches!(held, StudioEdit::Unresolved { token: held, .. } if *held == token),
        ) else {
            return;
        };
        match entry {
            Some(entry) => self.entries[index] = entry,
            None => {
                self.entries.remove(index);
                if index < self.cursor {
                    self.cursor -= 1;
                }
            }
        }
    }
    /// The index of the deletion `token` names while it can still be
    /// undone.
    pub fn undoable_deletion(&self, token: u64) -> Option<usize> {
        self.entries[..self.cursor].iter().position(
            |entry| matches!(entry, StudioEdit::ZoneDeleted(record) if record.token == token),
        )
    }
    pub fn complete(&mut self, redo: bool) {
        if redo && self.cursor < self.entries.len() {
            self.cursor += 1;
        } else if !redo && self.cursor > 0 {
            self.cursor -= 1;
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayoutReplay {
    pub zone_id: String,
    pub snapshot: LayoutEditorSnapshot,
    pub previous: LayoutEditorSnapshot,
    pub redo: bool,
}

/// A scene operation waiting for the one ahead of it to land.
enum Waiting {
    /// An undo (`false`) or redo (`true`), with the journal's recorded
    /// count it expects to find: the count when it was asked for, plus the
    /// edits queued ahead of it.
    Replay { redo: bool, recorded: u64 },
    /// An assignment; with a `point`, its arrivals center on it.
    Assign {
        zone_id: String,
        device_id: String,
        segments: Vec<String>,
        placements: Vec<MemberPlacementHint>,
        point: Option<NormalizedPosition>,
        done: Option<EditDone>,
    },
    Edit {
        changes: Vec<MemberEdit>,
        done: Option<EditDone>,
    },
    /// A zone to create; with a `plan`, the move into it as well.
    CreateZone {
        name: String,
        color: Option<String>,
        plan: Option<zones::PlanMove>,
        outcome: Box<dyn FnOnce(NewZone)>,
    },
    PatchZone {
        zone_id: String,
        after: PatchZoneRequest,
        failure: &'static str,
    },
    DeleteZone {
        zone_id: String,
        deleted: Callback<DeletedZone>,
    },
    /// The Undo bar's restore of the deletion this token names.
    UndoDeletion(u64),
    /// The scene's policy for outputs in no zone.
    UnassignedBehavior(UnassignedBehavior),
}

impl Waiting {
    /// Whether this operation records a journal entry when it lands.
    fn records(&self) -> bool {
        matches!(
            self,
            Self::Assign { .. }
                | Self::Edit { .. }
                | Self::CreateZone { .. }
                | Self::PatchZone { .. }
                | Self::DeleteZone { .. }
        )
    }

    /// Tell whoever waits on this operation that it will not run.
    fn stand_down(self) {
        match self {
            Self::Assign { done, .. } | Self::Edit { done, .. } => settle(done, None),
            Self::CreateZone { outcome, .. } => outcome(NewZone::NotCreated),
            Self::Replay { .. }
            | Self::PatchZone { .. }
            | Self::DeleteZone { .. }
            | Self::UndoDeletion(_)
            | Self::UnassignedBehavior(_) => {}
        }
    }
}

/// How a membership write came out.
enum Sent {
    /// It landed: the receipt to journal and the revision it reached.
    Landed {
        changes: Vec<MemberEdit>,
        revision: u64,
    },
    /// It did not apply; the failure is already reported.
    Failed,
    /// A scene switch overtook it; nothing may consume the reply.
    Superseded,
}

/// What a move into a zone sends.
pub(crate) enum MovePlan {
    /// Assign a device in no zone, centering its arrivals on `point` when
    /// one is given.
    Assign {
        device_id: String,
        segments: Vec<String>,
        placements: Vec<MemberPlacementHint>,
        point: Option<NormalizedPosition>,
    },
    /// Move existing outputs; each change's after state names the zone.
    Edit(Vec<MemberEdit>),
}

/// How the history reaches the layout editor: its commit queue, which
/// goes before any waiting operation, and its gestures, which a waiting
/// undo or redo never lands in the middle of.
#[derive(Clone, Copy)]
pub(crate) struct EditorHooks {
    pub commits_pending: Callback<(), bool>,
    pub drain_commits: Callback<()>,
    pub gesture_active: Callback<(), bool>,
}

/// How the history speaks through Studio's live region once an undo or
/// redo lands, and how it names the devices a step moved.
#[derive(Clone, Copy)]
pub(crate) struct Announcer {
    pub say: Callback<String>,
    /// A device's shown name and light segment count by its layout
    /// device id.
    pub device: Callback<String, Option<(String, usize)>>,
}

#[derive(Clone, Copy)]
pub(crate) struct StudioHistory {
    pub journal: RwSignal<StudioJournal>,
    /// A structural operation or a layout replay is in flight.
    pub busy: RwSignal<bool>,
    /// The layout commit queue is draining. Canvas edits go on.
    pub committing: RwSignal<bool>,
    pub pending_layout: RwSignal<Option<LayoutReplay>>,
    pub can_undo: Signal<bool>,
    pub can_redo: Signal<bool>,
    waiting: StoredValue<VecDeque<Waiting>, LocalStorage>,
    /// Hands out deletion tokens.
    tokens: StoredValue<u64>,
    editor: StoredValue<Option<EditorHooks>>,
    announcer: StoredValue<Option<Announcer>>,
    /// What to announce once the undo or redo in flight lands.
    replay_words: StoredValue<Option<String>>,
    /// Deletes whose outcome a lost connection hid, until a scene read
    /// settles them.
    unresolved: StoredValue<Vec<zones::Unresolved>>,
    scene: Signal<Option<api::SceneDocument>>,
    latest: RwSignal<Option<api::SceneDocument>>,
    generation: RwSignal<u64>,
    selected: RwSignal<Option<String>>,
    refresh: Callback<()>,
    /// The canvas size drop points are measured against.
    canvas: Memo<(u32, u32)>,
}

impl StudioHistory {
    pub fn new(
        scene: Signal<Option<api::SceneDocument>>,
        selected: RwSignal<Option<String>>,
        refresh: Callback<()>,
        canvas: Memo<(u32, u32)>,
    ) -> Self {
        let journal = RwSignal::new(StudioJournal::default());
        let busy = RwSignal::new(false);
        let committing = RwSignal::new(false);
        let waiting = StoredValue::new_local(VecDeque::<Waiting>::new());
        let pending_layout = RwSignal::new(None);
        let latest = RwSignal::new(None);
        let generation = RwSignal::new(0_u64);
        let unresolved = StoredValue::new(Vec::new());
        // A failed read leaves no scene for a moment; that is not a switch to
        // another scene, so the journal and the unresolved deletes stay.
        let scene_id = Memo::new(move |previous: Option<&Option<SceneId>>| {
            held_scene(
                scene.with(|s| s.as_ref().map(|s| s.id)),
                previous.copied().flatten(),
            )
        });
        Effect::new(move |_| {
            scene_id.get();
            journal.set(StudioJournal::default());
            unresolved.try_set_value(Vec::new());
            pending_layout.set(None);
            latest.set(None);
            generation.update(|g| *g = g.wrapping_add(1));
            busy.set(false);
            committing.set(false);
            let dropped = waiting.try_update_value(std::mem::take).unwrap_or_default();
            for operation in dropped {
                operation.stand_down();
            }
        });
        let history = Self {
            journal,
            busy,
            committing,
            waiting,
            tokens: StoredValue::new(0),
            editor: StoredValue::new(None),
            announcer: StoredValue::new(None),
            replay_words: StoredValue::new(None),
            unresolved,
            pending_layout,
            scene,
            latest,
            generation,
            selected,
            refresh,
            canvas,
            can_undo: Signal::derive(move || {
                !busy.get() && journal.with(|j| j.undo_edit().is_some())
            }),
            can_redo: Signal::derive(move || {
                !busy.get() && journal.with(|j| j.redo_edit().is_some())
            }),
        };
        // A delete no answer came back for settles once the connection is
        // back: sent again under its own key in its own daemon run, or read
        // from the scene once that run has ended.
        let socket = use_context::<crate::app::WsContext>()
            .map(|ws| (ws.connection_generation, ws.run_epoch));
        Effect::new(move |_| {
            scene.track();
            if let Some((opened, run)) = socket {
                opened.track();
                run.track();
            }
            if history.has_unresolved() {
                leptos::task::spawn_local(history.resolve_unresolved());
            }
        });
        history
    }

    /// The newest scene document Studio knows. `None` once Studio is gone,
    /// so a write finishing after it unmounts reads nothing instead of
    /// panicking on its disposed signals.
    pub fn current_scene(self) -> Option<api::SceneDocument> {
        let scene = self.scene.try_get_untracked().flatten()?;
        Some(
            self.latest
                .try_with_untracked(|latest| {
                    latest
                        .as_ref()
                        .filter(|latest| newer(latest, &scene))
                        .cloned()
                })
                .flatten()
                .unwrap_or(scene),
        )
    }

    pub fn current_scene_tracked(self) -> Option<api::SceneDocument> {
        let scene = self.scene.get()?;
        Some(
            self.latest
                .with(|latest| {
                    latest
                        .as_ref()
                        .filter(|latest| newer(latest, &scene))
                        .cloned()
                })
                .unwrap_or(scene),
        )
    }

    /// Record a zone write that landed at `revision` after its fence held
    /// against `base`. The fence admits no other change in between, so the
    /// scene at `revision` is `base` with that zone replaced, never an
    /// older cached copy with the zone patched in and stamped as current.
    pub fn record_saved_zone(
        self,
        base: &api::SceneDocument,
        zone: api::ZoneResource,
        revision: u64,
    ) {
        if revision <= base.revision
            || self
                .current_scene()
                .is_none_or(|current| current.id != base.id)
        {
            return;
        }
        let mut scene = base.clone();
        if let Some(current) = scene.zones.iter_mut().find(|current| current.id == zone.id) {
            *current = zone;
        }
        scene.revision = revision;
        self.adopt(scene);
    }

    /// Whether nothing holds the scene.
    fn is_free(self) -> bool {
        self.busy.try_get_untracked() == Some(false)
            && self.committing.try_get_untracked() == Some(false)
    }

    pub fn begin(self) -> Option<u64> {
        if !self.is_free() {
            return None;
        }
        self.busy.set(true);
        self.generation.update(|g| *g = g.wrapping_add(1));
        Some(self.generation.get_untracked())
    }

    pub fn is_current(self, generation: u64) -> bool {
        self.generation.try_get_untracked() == Some(generation)
    }

    pub fn finish(self, generation: u64) {
        if self.is_current(generation) {
            self.busy.set(false);
            self.next_waiting();
        }
    }

    /// Claim the scene for the layout commit queue.
    pub fn begin_commit(self) -> Option<u64> {
        if !self.is_free() {
            return None;
        }
        self.committing.set(true);
        self.generation.update(|g| *g = g.wrapping_add(1));
        Some(self.generation.get_untracked())
    }

    /// Release the scene once the commit queue drains.
    pub fn end_commit(self, generation: u64) {
        if self.is_current(generation) {
            self.committing.set(false);
            self.next_waiting();
        }
    }

    /// Connect the layout editor; `None` disconnects it.
    pub fn attach_editor(self, hooks: Option<EditorHooks>) {
        self.editor.try_set_value(hooks);
    }

    /// Connect the live region; `None` disconnects it.
    pub fn attach_announcer(self, announcer: Option<Announcer>) {
        self.announcer.try_set_value(announcer);
    }

    /// Word the announcement for replaying `edit`, against the names the
    /// scene and the device registry have now.
    fn prepare_words(self, edit: &StudioEdit, redo: bool) {
        let scene = self.current_scene();
        let announcer = self.announcer.try_get_value().flatten();
        let zone_lookup = |id: &str| {
            scene.as_ref().and_then(|scene| {
                scene
                    .zones
                    .iter()
                    .find(|zone| zone.id.to_string() == id)
                    .map(|zone| (zone.name.clone(), zone.role))
            })
        };
        let device = |id: &str| {
            announcer.and_then(|announcer| announcer.device.try_run(id.to_owned()).flatten())
        };
        let words = replay_announcement(edit, redo, &zone_lookup, &device);
        self.replay_words.try_set_value(words);
    }

    /// Say what the undo or redo that just landed did.
    fn announce_replay(self) {
        let words = self.replay_words.try_update_value(Option::take).flatten();
        if let (Some(words), Some(announcer)) = (words, self.announcer.try_get_value().flatten()) {
            let _ = announcer.say.try_run(words);
        }
    }

    fn editor_hooks(self) -> Option<EditorHooks> {
        self.editor.try_get_value().flatten()
    }

    /// Whether a canvas gesture is running. Every operation waits it out:
    /// one that took the scene mid-gesture would lock the canvas, and the
    /// gesture's release could no longer commit.
    fn gesture_running(self) -> bool {
        self.editor_hooks()
            .is_some_and(|editor| editor.gesture_active.try_run(()).unwrap_or(false))
    }

    /// Run `operation` now when nothing holds the scene, otherwise after
    /// everything already waiting.
    fn when_free(self, operation: Waiting) {
        let queued = self
            .waiting
            .try_with_value(|waiting| !waiting.is_empty())
            .unwrap_or(false);
        if self.is_free() && !queued && !self.gesture_running() && !self.commits_pending() {
            self.run(operation);
        } else {
            self.waiting
                .try_update_value(|waiting| waiting.push_back(operation));
            self.next_waiting();
        }
    }

    /// Whether layout commits wait or are being written. They go before any
    /// operation, so an operation never acts on a zone an edit is still
    /// landing in.
    fn commits_pending(self) -> bool {
        self.editor_hooks()
            .is_some_and(|editor| editor.commits_pending.try_run(()).unwrap_or(false))
    }

    /// Pick the queue up again after a canvas gesture ends.
    pub fn resume(self) {
        self.next_waiting();
    }

    /// Start waiting operations, in order, while the scene stays free.
    /// Layout commits go first, so an operation always acts on the scene
    /// with every finished edit (an undo included) already written.
    fn next_waiting(self) {
        while self.is_free() {
            if self.commits_pending() {
                // The writer takes them; it hands the scene back as it goes
                // idle, and the queue picks up from there.
                if let Some(editor) = self.editor_hooks() {
                    let _ = editor.drain_commits.try_run(());
                }
                return;
            }
            if self.gesture_running() {
                return;
            }
            let Some(operation) = self.waiting.try_update_value(VecDeque::pop_front).flatten()
            else {
                return;
            };
            self.run(operation);
        }
    }

    fn run(self, operation: Waiting) {
        match operation {
            Waiting::Replay { redo, recorded } => self.replay_now(redo, recorded),
            Waiting::Assign {
                zone_id,
                device_id,
                segments,
                placements,
                point: None,
                done,
            } => self.assign_device_now(zone_id, device_id, segments, placements, done),
            Waiting::Assign {
                zone_id,
                device_id,
                segments,
                placements,
                point: Some(point),
                done,
            } => self.assign_device_at_now(zone_id, device_id, segments, placements, point, done),
            Waiting::Edit { changes, done } => self.edit_members_now(changes, done),
            Waiting::CreateZone {
                name,
                color,
                plan,
                outcome,
            } => self.create_zone_now(name, color, plan, outcome),
            Waiting::PatchZone {
                zone_id,
                after,
                failure,
            } => self.patch_zone_now(zone_id, after, failure),
            Waiting::DeleteZone { zone_id, deleted } => self.delete_zone_now(zone_id, deleted),
            Waiting::UndoDeletion(token) => self.undo_deletion_now(token),
            Waiting::UnassignedBehavior(behavior) => self.set_unassigned_behavior_now(behavior),
        }
    }

    pub fn complete_layout(self, redo: bool) {
        self.journal.update(|j| j.complete(redo));
        self.announce_replay();
        self.pending_layout.set(None);
        self.busy.set(false);
        self.next_waiting();
    }

    pub fn layout_replay_for_zone(self, zone_id: Option<&str>) -> Option<LayoutReplay> {
        let replay = self.pending_layout.get()?;
        if zone_id == Some(replay.zone_id.as_str()) {
            return Some(replay);
        }
        // Selection can change between the toolbar event and the provider's
        // effect. Cancel the unapplied step without consuming its history.
        self.pending_layout.set(None);
        self.busy.set(false);
        toasts::toast_info("Layout history cancelled after the selected zone changed");
        self.next_waiting();
        None
    }

    pub fn record_layout(
        self,
        zone_id: String,
        before: LayoutEditorSnapshot,
        after: LayoutEditorSnapshot,
    ) {
        if before != after {
            self.journal.update(|j| {
                j.record(StudioEdit::Layout {
                    zone_id,
                    before: Box::new(before),
                    after: Box::new(after),
                })
            });
        }
    }

    /// Treat `scene` as the live document when it is newer than the shared
    /// one, so an edit can follow a zone created a moment ago.
    pub fn adopt(self, scene: api::SceneDocument) {
        let take = self.current_scene().is_none_or(|current| {
            current.id != scene.id
                || current.run_epoch != scene.run_epoch
                || scene.revision > current.revision
        });
        if take {
            self.latest.set(Some(scene));
        }
    }

    pub fn assign_device(
        self,
        zone_id: String,
        device_id: String,
        segments: Vec<String>,
        placements: Vec<MemberPlacementHint>,
        done: Option<EditDone>,
    ) {
        self.when_free(Waiting::Assign {
            zone_id,
            device_id,
            segments,
            placements,
            point: None,
            done,
        });
    }

    /// Assign a device and center the outputs it gains on `point`, as one
    /// undo step. The daemon mints the outputs, so the second write moves
    /// them once their geometry is known.
    pub fn assign_device_at(
        self,
        zone_id: String,
        device_id: String,
        segments: Vec<String>,
        placements: Vec<MemberPlacementHint>,
        point: NormalizedPosition,
        done: Option<EditDone>,
    ) {
        self.when_free(Waiting::Assign {
            zone_id,
            device_id,
            segments,
            placements,
            point: Some(point),
            done,
        });
    }

    fn assign_device_now(
        self,
        zone_id: String,
        device_id: String,
        segments: Vec<String>,
        placements: Vec<MemberPlacementHint>,
        done: Option<EditDone>,
    ) {
        let Some((scene, request)) = self.assignment(zone_id, device_id, segments, placements)
        else {
            settle(done, None);
            return;
        };
        self.submit(request, Fence::of(&scene), done);
    }

    fn assign_device_at_now(
        self,
        zone_id: String,
        device_id: String,
        segments: Vec<String>,
        placements: Vec<MemberPlacementHint>,
        point: NormalizedPosition,
        done: Option<EditDone>,
    ) {
        let Some((scene, request)) = self.assignment(zone_id, device_id, segments, placements)
        else {
            settle(done, None);
            return;
        };
        self.submit_with(request, Fence::of(&scene), Some(point), done);
    }

    fn assignment(
        self,
        zone_id: String,
        device_id: String,
        segments: Vec<String>,
        placements: Vec<MemberPlacementHint>,
    ) -> Option<(api::SceneDocument, EditMembersRequest)> {
        let scene = self.current_scene()?;
        let zone = scene
            .zones
            .iter()
            .find(|zone| zone.id.to_string() == zone_id)?;
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
        Some((scene, request))
    }

    /// Set the scene's policy for outputs in no zone, in turn with every
    /// other scene write.
    pub fn set_unassigned_behavior(self, behavior: UnassignedBehavior) {
        self.when_free(Waiting::UnassignedBehavior(behavior));
    }

    fn set_unassigned_behavior_now(self, behavior: UnassignedBehavior) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        if scene.unassigned_behavior == behavior {
            return;
        }
        let Some(generation) = self.begin() else {
            return;
        };
        leptos::task::spawn_local(async move {
            let key = keyed::new_key();
            let result = keyed::send(&scene.run_epoch, || {
                api::zones::update_unassigned_behavior(&behavior, scene.revision, &key)
            })
            .await
            .into_result();
            if self.is_current(generation) {
                match result {
                    Ok(api::zones::ZoneOutcome::Applied(_)) => {}
                    Ok(api::zones::ZoneOutcome::Stale { .. }) => {
                        toasts::toast_error("Scene changed elsewhere. Reloaded, try again.");
                    }
                    Err(error) => toasts::toast_error(&format!("Policy update failed: {error}")),
                }
            }
            self.release(generation).await;
        });
    }

    /// Remove outputs from the zone `zone_id` in one membership edit.
    pub fn remove_outputs(self, zone_id: &str, output_ids: &[String]) {
        use hypercolor_types::api::scene::MemberState;
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(zone) = scene
            .zones
            .iter()
            .find(|zone| zone.id.to_string() == zone_id)
        else {
            return;
        };
        let changes = api::zone_outputs(zone)
            .into_iter()
            .enumerate()
            .filter(|(_, output)| output_ids.contains(&output.id))
            .map(|(index, output)| MemberEdit {
                before: Some(MemberState {
                    zone_id: zone.id,
                    output,
                    index,
                }),
                after: None,
            })
            .collect();
        self.edit_members(changes, None);
    }

    pub fn edit_members(self, changes: Vec<MemberEdit>, done: Option<EditDone>) {
        if changes.is_empty() {
            settle(done, None);
            return;
        }
        self.when_free(Waiting::Edit { changes, done });
    }

    /// Send a membership edit against the scene as it stands now. Its
    /// preconditions are read afresh, since a layout commit may have moved
    /// an output while the edit waited; ownership and binding must hold.
    fn edit_members_now(self, changes: Vec<MemberEdit>, done: Option<EditDone>) {
        let Some(scene) = self.current_scene() else {
            settle(done, None);
            return;
        };
        let Ok(changes) = rebase_preconditions(&scene, changes) else {
            toasts::toast_error("Assignment changed elsewhere; nothing was changed");
            settle(done, None);
            return;
        };
        self.submit(
            EditMembersRequest {
                scene_id: scene.id,
                changes,
                assignment: None,
            },
            Fence::of(&scene),
            done,
        );
    }

    fn submit(self, request: EditMembersRequest, fence: Fence, done: Option<EditDone>) {
        self.submit_with(request, fence, None, done);
    }

    /// Send a membership write as one operation and journal what landed.
    fn submit_with(
        self,
        request: EditMembersRequest,
        fence: Fence,
        point: Option<NormalizedPosition>,
        done: Option<EditDone>,
    ) {
        let Some(generation) = self.begin() else {
            settle(done, None);
            return;
        };
        leptos::task::spawn_local(async move {
            let applied = match self.send_members(generation, request, fence, point).await {
                Sent::Superseded => {
                    self.settle_superseded(done);
                    return;
                }
                Sent::Failed => None,
                Sent::Landed { changes, revision } => {
                    if !changes.is_empty() {
                        self.journal
                            .update(|j| j.record(StudioEdit::Members(changes)));
                    }
                    toasts::toast_success("Assignment saved");
                    Some(revision)
                }
            };
            self.close(generation, done, applied);
        });
    }

    /// Send one membership write; for an assignment with a `point`, follow
    /// it with the write that centers its arrivals there, folded into one
    /// receipt so one undo takes back both. A stale reply resyncs the
    /// scene before reporting, so whatever waits next reads it fresh.
    async fn send_members(
        self,
        generation: u64,
        request: EditMembersRequest,
        fence: Fence,
        point: Option<NormalizedPosition>,
    ) -> Sent {
        let scene_id = request.scene_id;
        let result = edit_members(&request, &fence).await;
        if !self.is_current(generation) {
            return Sent::Superseded;
        }
        let response = match result {
            Ok(api::zones::ZoneOutcome::Applied(response)) => response,
            outcome => {
                report_failure(&outcome);
                if matches!(outcome, Ok(api::zones::ZoneOutcome::Stale { .. })) {
                    self.resync(generation).await;
                }
                return Sent::Failed;
            }
        };
        let mut landed = response.document.revision;
        let mut changes = response.changes.clone();
        let canvas = self.canvas.get_untracked();
        let placed = point.map_or_else(Vec::new, |point| {
            center_arrivals(&response.changes, point, canvas)
        });
        self.accept(response);
        if !placed.is_empty() {
            let request = EditMembersRequest {
                scene_id,
                changes: placed,
                assignment: None,
            };
            let result = edit_members(&request, &fence.at(landed)).await;
            if !self.is_current(generation) {
                return Sent::Superseded;
            }
            match result {
                Ok(api::zones::ZoneOutcome::Applied(response)) => {
                    landed = response.document.revision;
                    fold_placements(&mut changes, &response.changes);
                    self.accept(response);
                }
                _ => {
                    toasts::toast_info("Assigned, but it could not be placed where you dropped it")
                }
            }
        }
        Sent::Landed {
            changes,
            revision: landed,
        }
    }

    /// Read the live scene after a stale reply, so the next operation does
    /// not fail against the same old revision.
    async fn resync(self, generation: u64) {
        if let Ok(live) = api::fetch_active_scene().await
            && self.is_current(generation)
        {
            self.adopt(live);
        }
    }

    /// End a membership write: tell the caller the revision it landed at,
    /// if it applied, then refresh the scene and release the history. The
    /// caller hears first, so results announce in the order they land.
    fn close(self, generation: u64, done: Option<EditDone>, applied: Option<u64>) {
        settle(done, applied);
        self.refresh.run(());
        self.finish(generation);
    }

    /// Whether Studio is still there to hear an outcome.
    fn alive(self) -> bool {
        self.generation.try_get_untracked().is_some()
    }

    /// Settle a write a scene switch overtook, while Studio is still
    /// there to hear it.
    fn settle_superseded(self, done: Option<EditDone>) {
        if self.alive() {
            settle(done, None);
        }
    }

    fn accept(self, response: EditMembersResponse) {
        // The membership write placed these outputs; a leftover layout edit
        // for them must not land over it.
        let moved: Vec<String> = response
            .changes
            .iter()
            .filter_map(|change| change.after.as_ref().or(change.before.as_ref()))
            .map(|state| state.output.id.clone())
            .collect();
        crate::components::layout_builder::writer::forget(response.document.id, &moved);
        self.latest.set(Some(response.document));
    }

    /// Let go of the newest document history knew, as when the daemon may
    /// have started a new run whose revisions start over below it.
    pub fn forget_latest(self) {
        self.latest.try_set(None);
    }

    /// The journal index an undo (`false`) or redo (`true`) replays.
    fn replay_index(self, redo: bool) -> usize {
        self.journal.with_untracked(|j| {
            if redo {
                j.cursor
            } else {
                j.cursor.saturating_sub(1)
            }
        })
    }

    pub fn replay(self, redo: bool) {
        // Edits already queued count as asked for before this; an undo
        // pressed after clicking Add means to undo that Add.
        let ahead = self
            .waiting
            .try_with_value(|waiting| {
                waiting
                    .iter()
                    .filter(|operation| operation.records())
                    .count()
            })
            .unwrap_or(0);
        let recorded = self
            .journal
            .with_untracked(StudioJournal::recorded)
            .wrapping_add(u64::try_from(ahead).unwrap_or(u64::MAX));
        self.when_free(Waiting::Replay { redo, recorded });
    }

    fn replay_now(self, redo: bool, recorded: u64) {
        // An edit made while this waited is newer than what it was asked
        // to reverse; undoing that instead would surprise. A queued edit
        // that failed records nothing, which is no reason to stand down.
        if self.journal.with_untracked(StudioJournal::recorded) > recorded {
            toasts::toast_info(if redo {
                "Redo skipped: a newer change came first"
            } else {
                "Undo skipped: a newer change came first"
            });
            return;
        }
        let edit = self
            .journal
            .with_untracked(|j| if redo { j.redo_edit() } else { j.undo_edit() }.cloned());
        let Some(edit) = edit else {
            return;
        };
        self.prepare_words(&edit, redo);
        match edit {
            StudioEdit::Layout {
                zone_id,
                before,
                after,
            } => {
                if !self.current_scene().is_some_and(|scene| {
                    scene.zones.iter().any(|zone| {
                        zone.id.to_string() == zone_id
                            && zone.role != hypercolor_types::scene::ZoneRole::Display
                    })
                }) {
                    toasts::toast_error("The edited zone is no longer available");
                    return;
                }
                self.busy.set(true);
                self.selected.set(Some(zone_id.clone()));
                self.pending_layout.set(Some(LayoutReplay {
                    zone_id,
                    snapshot: if redo {
                        after.as_ref().clone()
                    } else {
                        before.as_ref().clone()
                    },
                    previous: if redo { *before } else { *after },
                    redo,
                }));
            }
            StudioEdit::Members(changes) if changes.is_empty() => {
                self.journal.update(|j| j.complete(redo));
            }
            StudioEdit::ZoneCreated {
                zone_id,
                name,
                color,
                filled,
            } => {
                let index = self.replay_index(redo);
                self.replay_zone_created(index, redo, zone_id, name, color, filled);
            }
            StudioEdit::ZonePatched {
                zone_id,
                before,
                after,
            } => {
                self.replay_zone_patched(redo, zone_id, if redo { after } else { before });
            }
            StudioEdit::ZoneDeleted(record) => {
                let index = self.replay_index(redo);
                self.replay_zone_deleted_step(index, redo, *record);
            }
            StudioEdit::Unresolved { name, .. } => {
                toasts::toast_info(&format!(
                    "Still waiting to learn whether {name} was deleted. Undo and redo go on once the scene loads again."
                ));
            }
            StudioEdit::Members(changes) => {
                let Some(scene) = self.current_scene() else {
                    return;
                };
                let changes = changes
                    .into_iter()
                    .map(|change| {
                        if redo {
                            change
                        } else {
                            MemberEdit {
                                before: change.after,
                                after: change.before,
                            }
                        }
                    })
                    .collect::<Vec<_>>();
                let Ok(changes) = prepare_members_replay(&scene, changes) else {
                    toasts::toast_error("Assignment changed elsewhere; history was not applied");
                    return;
                };
                let Some(generation) = self.begin() else {
                    return;
                };
                leptos::task::spawn_local(async move {
                    let request = EditMembersRequest {
                        scene_id: scene.id,
                        changes,
                        assignment: None,
                    };
                    let result = edit_members(&request, &Fence::of(&scene)).await;
                    if !self.is_current(generation) {
                        return;
                    }
                    match result {
                        Ok(api::zones::ZoneOutcome::Applied(response)) => {
                            if self.is_current(generation)
                                && self.scene.with_untracked(|s| {
                                    s.as_ref().is_some_and(|s| s.id == scene.id)
                                })
                            {
                                self.journal.update(|j| {
                                    let index = if redo { j.cursor } else { j.cursor - 1 };
                                    let changes = response
                                        .changes
                                        .iter()
                                        .cloned()
                                        .map(|change| {
                                            if redo {
                                                change
                                            } else {
                                                MemberEdit {
                                                    before: change.after,
                                                    after: change.before,
                                                }
                                            }
                                        })
                                        .collect();
                                    j.entries[index] = StudioEdit::Members(changes);
                                    j.complete(redo);
                                });
                                self.announce_replay();
                                self.accept(response);
                                toasts::toast_success(if redo {
                                    "Assignment redone and saved"
                                } else {
                                    "Assignment undone and saved"
                                });
                            }
                        }
                        Ok(api::zones::ZoneOutcome::Stale { .. }) => toasts::toast_error(
                            "Scene changed elsewhere; history was not applied. Try again.",
                        ),
                        Err(error) => {
                            toasts::toast_error(&format!("History could not be applied: {error}"))
                        }
                    }
                    self.refresh.run(());
                    self.finish(generation);
                });
            }
        }
    }
}

fn settle(done: Option<EditDone>, applied: Option<u64>) {
    if let Some(done) = done {
        done(applied);
    }
}

/// Whether `latest`, a document history adopted, is newer than the shared
/// `scene`. Revisions only compare within one daemon run, so a document
/// of another run never is.
fn newer(latest: &api::SceneDocument, scene: &api::SceneDocument) -> bool {
    latest.id == scene.id && latest.run_epoch == scene.run_epoch && latest.revision > scene.revision
}

/// Send one membership edit fenced at `fence`, under an idempotency key of
/// its own that every retry of it reuses. No answer, or one from another
/// daemon run, reads as an error.
pub(super) async fn edit_members(
    request: &EditMembersRequest,
    fence: &Fence,
) -> api::ApiResult<api::zones::ZoneOutcome<EditMembersResponse>> {
    let key = keyed::new_key();
    keyed::send(&fence.run, || {
        api::zones::edit_members(request, fence.revision, &key)
    })
    .await
    .into_result()
}

fn report_failure<T>(outcome: &api::ApiResult<api::zones::ZoneOutcome<T>>) {
    match outcome {
        Ok(api::zones::ZoneOutcome::Applied(_)) => {}
        Ok(api::zones::ZoneOutcome::Stale { .. }) => {
            toasts::toast_error("Scene changed elsewhere; assignment was not saved. Try again.");
        }
        Err(error) => toasts::toast_error(&format!("Assignment failed: {error}")),
    }
}

/// The placement edits that center an assignment's arrivals on `point`.
/// Each arrival keeps its zone, index, and relation to the others.
fn center_arrivals(
    changes: &[MemberEdit],
    point: NormalizedPosition,
    canvas: (u32, u32),
) -> Vec<MemberEdit> {
    let arrivals: Vec<_> = changes
        .iter()
        .filter_map(|change| change.after.clone())
        .collect();
    let footprints = arrivals.iter().map(|state| Footprint::of(&state.output));
    let Some((dx, dy)) = placement::offset_to_center(footprints, point, canvas) else {
        return Vec::new();
    };
    if dx.abs() < f32::EPSILON && dy.abs() < f32::EPSILON {
        return Vec::new();
    }
    arrivals
        .into_iter()
        .map(|state| {
            let mut placed = state.clone();
            placed.output.position =
                NormalizedPosition::new(state.output.position.x + dx, state.output.position.y + dy);
            MemberEdit {
                before: Some(state),
                after: Some(placed),
            }
        })
        .collect()
}

/// Fold a follow-up placement receipt into the assignment it placed, so
/// one undo takes back both.
fn fold_placements(changes: &mut [MemberEdit], placed: &[MemberEdit]) {
    for change in changes {
        let Some(after) = change.after.as_mut() else {
            continue;
        };
        if let Some(moved) = placed
            .iter()
            .filter_map(|edit| edit.after.as_ref())
            .find(|state| state.output.id == after.output.id)
        {
            *after = moved.clone();
        }
    }
}

/// Read each change's preconditions from the live scene: where the output
/// sits now and at what index. Ownership and binding must still match.
/// The changes' outcomes are left as asked.
fn rebase_preconditions(
    scene: &api::SceneDocument,
    changes: Vec<MemberEdit>,
) -> Result<Vec<MemberEdit>, String> {
    changes
        .into_iter()
        .map(|mut change| {
            if let Some(before) = change.before.as_mut() {
                let (index, output) = live_member(scene, before)?;
                before.output = output;
                before.index = index;
            }
            Ok(change)
        })
        .collect()
}

/// An output's live index and state in the zone a precondition names,
/// provided its binding is unchanged.
fn live_member(
    scene: &api::SceneDocument,
    state: &hypercolor_types::api::scene::MemberState,
) -> Result<(usize, hypercolor_types::spatial::Output), String> {
    let zone = scene
        .zones
        .iter()
        .find(|zone| zone.id == state.zone_id)
        .ok_or("Zone no longer exists")?;
    let (index, output) = api::zone_outputs(zone)
        .into_iter()
        .enumerate()
        .find(|(_, output)| output.id == state.output.id)
        .ok_or("Output no longer belongs to the zone")?;
    if output.device_id != state.output.device_id || output.zone_name != state.output.zone_name {
        return Err("Output binding changed".into());
    }
    Ok((index, output))
}

/// Resolve live preconditions while retaining unrelated placement edits.
/// Ownership and binding must still match the operation being reversed.
/// An output still sitting where the operation left it returns to the
/// placement the operation recorded (a move lands its outputs per the rig,
/// so its undo puts them back); one placed since keeps its newer spot.
pub fn prepare_members_replay(
    scene: &api::SceneDocument,
    changes: Vec<MemberEdit>,
) -> Result<Vec<MemberEdit>, String> {
    changes
        .into_iter()
        .map(|mut change| {
            if let Some(before) = change.before.as_mut() {
                let (index, output) = live_member(scene, before)?;
                let untouched = Footprint::of(&output) == Footprint::of(&before.output);
                before.output = output.clone();
                before.index = index;
                if let Some(after) = change.after.as_mut() {
                    let recorded = Footprint::of(&after.output);
                    after.output = output;
                    if untouched {
                        recorded.apply(&mut after.output);
                    }
                }
            }
            Ok(change)
        })
        .collect()
}

#[cfg(test)]
mod replay_tests {
    use super::*;

    fn snapshot() -> LayoutEditorSnapshot {
        LayoutEditorSnapshot {
            zones: Vec::new(),
            selected_zone_ids: Default::default(),
            compound_depth: crate::compound_selection::CompoundDepth::Root,
        }
    }

    fn history(
        scene: Option<api::SceneDocument>,
        busy: bool,
        pending_layout: Option<LayoutReplay>,
        selected: &str,
    ) -> StudioHistory {
        StudioHistory {
            journal: RwSignal::new(StudioJournal::default()),
            busy: RwSignal::new(busy),
            committing: RwSignal::new(false),
            waiting: StoredValue::new_local(VecDeque::new()),
            tokens: StoredValue::new(0),
            editor: StoredValue::new(None),
            announcer: StoredValue::new(None),
            replay_words: StoredValue::new(None),
            unresolved: StoredValue::new(Vec::new()),
            pending_layout: RwSignal::new(pending_layout),
            can_undo: Signal::stored(true),
            can_redo: Signal::stored(false),
            scene: Signal::stored(scene),
            latest: RwSignal::new(None),
            generation: RwSignal::new(1),
            selected: RwSignal::new(Some(selected.to_owned())),
            refresh: Callback::new(|()| {}),
            canvas: Memo::new(|_| (640, 480)),
        }
    }

    fn record_layout_edit(history: StudioHistory, zone_id: &str) {
        history.journal.update(|journal| {
            journal.record(StudioEdit::Layout {
                zone_id: zone_id.into(),
                before: Box::new(snapshot()),
                after: Box::new(snapshot()),
            })
        });
    }

    #[test]
    fn changed_selection_cancels_pending_replay_without_consuming_history() {
        let owner = Owner::new();
        owner.with(|| {
            let pending = LayoutReplay {
                zone_id: "one".into(),
                snapshot: snapshot(),
                previous: snapshot(),
                redo: false,
            };
            let history = history(None, true, Some(pending), "two");
            record_layout_edit(history, "one");
            assert!(history.layout_replay_for_zone(Some("one")).is_some());
            assert!(history.busy.get_untracked());
            assert!(history.layout_replay_for_zone(Some("two")).is_none());
            assert!(!history.busy.get_untracked());
            assert!(history.pending_layout.get_untracked().is_none());
            assert!(
                history
                    .journal
                    .with_untracked(|journal| journal.undo_edit().is_some())
            );
            assert!(
                history
                    .journal
                    .with_untracked(|journal| journal.redo_edit().is_none())
            );
        });
        owner.cleanup();
    }

    #[test]
    fn undo_presses_during_layout_commits_each_wait_their_turn() {
        let owner = Owner::new();
        owner.with(|| {
            let zone = "11111111-1111-4111-8111-111111111111";
            let scene: api::SceneDocument = serde_json::from_value(serde_json::json!({
                "id": "33333333-3333-4333-8333-333333333333", "name": "Desk", "kind": "named",
                "is_default": false, "revision": 4,
                "zones": [{ "id": zone, "name": "Main", "role": "primary", "enabled": true,
                    "brightness": 1, "members": [], "layers": [] }],
            }))
            .expect("valid scene fixture");
            let history = history(Some(scene), false, None, zone);
            record_layout_edit(history, zone);
            record_layout_edit(history, zone);

            let generation = history.begin_commit().expect("scene is free");
            assert!(
                !history.busy.get_untracked(),
                "a draining commit leaves the canvas editable"
            );
            history.replay(false);
            history.replay(false);
            assert!(history.pending_layout.get_untracked().is_none());

            history.end_commit(generation);
            assert!(history.pending_layout.get_untracked().is_some());
            assert!(
                history.busy.get_untracked(),
                "the first undo holds the scene until it applies"
            );

            history.complete_layout(false);
            assert!(
                history.pending_layout.get_untracked().is_some(),
                "the second undo runs once the first lands"
            );
            history.complete_layout(false);
            assert!(
                history
                    .journal
                    .with_untracked(|journal| journal.undo_edit().is_none())
            );
        });
        owner.cleanup();
    }

    #[test]
    fn layout_commits_go_before_waiting_operations() {
        let owner = Owner::new();
        owner.with(|| {
            let zone = "11111111-1111-4111-8111-111111111111";
            let history = history(None, true, None, zone);
            record_layout_edit(history, zone);
            let pending = RwSignal::new(true);
            history.attach_editor(Some(EditorHooks {
                commits_pending: Callback::new(move |()| pending.get_untracked()),
                drain_commits: Callback::new(move |()| {
                    pending.set(false);
                    let _ = history.begin_commit();
                }),
                gesture_active: Callback::new(|()| false),
            }));
            // An undo asked for while a structural operation runs.
            history.replay(false);
            history.finish(1);
            assert!(
                history.committing.get_untracked(),
                "the queued commit took the scene first"
            );
            assert!(history.pending_layout.get_untracked().is_none());
            let generation = history.generation.get_untracked();
            history.end_commit(generation);
            assert!(
                history.pending_layout.get_untracked().is_none(),
                "the undo found no scene to replay against and stood down"
            );
            assert!(history.waiting.with_value(VecDeque::is_empty));
        });
        owner.cleanup();
    }

    #[test]
    fn an_undo_pressed_after_a_queued_edit_undoes_that_edit() {
        let owner = Owner::new();
        owner.with(|| {
            let zone = "11111111-1111-4111-8111-111111111111";
            let history = history(None, true, None, zone);
            record_layout_edit(history, zone);
            history.waiting.update_value(|waiting| {
                waiting.push_back(Waiting::Edit {
                    changes: Vec::new(),
                    done: None,
                });
            });
            history.replay(false);
            let expected = history.waiting.with_value(|waiting| match waiting.back() {
                Some(Waiting::Replay { recorded, .. }) => Some(*recorded),
                _ => None,
            });
            assert_eq!(
                expected,
                Some(history.journal.with_untracked(StudioJournal::recorded) + 1),
                "the queued edit counts as asked for before the undo"
            );
        });
        owner.cleanup();
    }

    #[test]
    fn an_undo_pressed_after_a_queued_zone_delete_undoes_that_delete() {
        let owner = Owner::new();
        owner.with(|| {
            let zone = "11111111-1111-4111-8111-111111111111";
            let history = history(None, true, None, zone);
            history.delete_zone(zone.to_owned(), Callback::new(|_| {}));
            history.undo_deletion(1);
            history.replay(false);
            let expected = history.waiting.with_value(|waiting| match waiting.back() {
                Some(Waiting::Replay { recorded, .. }) => Some(*recorded),
                _ => None,
            });
            assert_eq!(
                expected,
                Some(history.journal.with_untracked(StudioJournal::recorded) + 1),
                "the queued delete counts and the Undo bar's restore does not"
            );
        });
        owner.cleanup();
    }

    #[test]
    fn a_queued_zone_create_that_stands_down_hears_it_was_not_made() {
        let owner = Owner::new();
        owner.with(|| {
            let zone = "11111111-1111-4111-8111-111111111111";
            let history = history(None, true, None, zone);
            let heard = StoredValue::new(Vec::new());
            history.create_zone(
                "Desk".to_owned(),
                None,
                Box::new(move |created| heard.update_value(|heard| heard.push(created.is_some()))),
            );
            assert!(
                heard.with_value(Vec::is_empty),
                "the create waits while the scene is held"
            );
            let dropped = history
                .waiting
                .try_update_value(std::mem::take)
                .unwrap_or_default();
            for operation in dropped {
                operation.stand_down();
            }
            assert_eq!(heard.get_value(), vec![false]);
        });
        owner.cleanup();
    }

    fn two_zone_scene(revision: u64, keyboard_x: f32) -> api::SceneDocument {
        serde_json::from_value(serde_json::json!({
            "id": "33333333-3333-4333-8333-333333333333", "name": "Desk", "kind": "named",
            "is_default": false, "revision": revision,
            "zones": [
                { "id": "11111111-1111-4111-8111-111111111111", "name": "Main",
                  "role": "primary", "enabled": true, "brightness": 1, "layers": [],
                  "members": [{ "id": "stand", "device_id": "usb:stand", "segment": "Main",
                      "name": "Stand" }],
                  "layout": { "placements": [{ "member": "stand",
                      "position": { "x": 0.2, "y": 0.5 }, "size": { "x": 0.1, "y": 0.1 },
                      "topology": { "type": "strip", "count": 15,
                          "direction": "left_to_right" } }] } },
                { "id": "22222222-2222-4222-8222-222222222222", "name": "Keyboard",
                  "role": "custom", "enabled": true, "brightness": 1, "layers": [],
                  "members": [{ "id": "keys", "device_id": "usb:keys", "segment": "Main",
                      "name": "Keys" }],
                  "layout": { "placements": [{ "member": "keys",
                      "position": { "x": keyboard_x, "y": 0.5 }, "size": { "x": 0.1, "y": 0.1 },
                      "topology": { "type": "strip", "count": 15,
                          "direction": "left_to_right" } }] } },
            ],
        }))
        .expect("valid scene fixture")
    }

    #[test]
    fn a_retried_zone_write_records_the_scene_it_was_fenced_against() {
        let owner = Owner::new();
        owner.with(|| {
            let cached = two_zone_scene(4, 0.2);
            let history = history(
                Some(cached),
                false,
                None,
                "11111111-1111-4111-8111-111111111111",
            );
            // Another client moved Keyboard; the retry fenced on that scene.
            let fresh = two_zone_scene(6, 0.7);
            let mut main = fresh.zones[0].clone();
            main.name = "Main written".to_owned();
            history.record_saved_zone(&fresh, main, 7);
            let current = history.current_scene().expect("scene");
            assert_eq!(current.revision, 7);
            assert_eq!(current.zones[0].name, "Main written");
            assert_eq!(
                current.zones[1], fresh.zones[1],
                "the other zone is the one the write was fenced against"
            );
        });
        owner.cleanup();
    }

    #[test]
    fn a_history_outliving_studio_reads_no_scene_instead_of_panicking() {
        let owner = Owner::new();
        let history = owner.with(|| {
            let zone = "11111111-1111-4111-8111-111111111111";
            let scene: api::SceneDocument = serde_json::from_value(serde_json::json!({
                "id": "33333333-3333-4333-8333-333333333333", "name": "Desk", "kind": "named",
                "is_default": false, "revision": 4,
                "zones": [{ "id": zone, "name": "Main", "role": "primary", "enabled": true,
                    "brightness": 1, "members": [], "layers": [] }],
            }))
            .expect("valid scene fixture");
            let history = history(Some(scene), false, None, zone);
            assert!(history.current_scene().is_some());
            history
        });
        // Studio unmounts while a write is still awaiting its reply.
        owner.cleanup();
        assert!(history.current_scene().is_none());
        history.select_away("11111111-1111-4111-8111-111111111111", None);
        history.prepare_words(&StudioEdit::Members(Vec::new()), false);
        history.announce_replay();
        assert!(!history.is_current(1));
    }

    #[test]
    fn the_unassigned_policy_waits_its_turn_behind_a_running_write() {
        let owner = Owner::new();
        owner.with(|| {
            let history = history(None, true, None, "11111111-1111-4111-8111-111111111111");
            history.set_unassigned_behavior(UnassignedBehavior::Off);
            assert!(
                history.waiting.with_value(|waiting| waiting.len() == 1),
                "the policy write queues instead of racing the write in flight"
            );
            history.replay(false);
            let expected = history.waiting.with_value(|waiting| match waiting.back() {
                Some(Waiting::Replay { recorded, .. }) => Some(*recorded),
                _ => None,
            });
            assert_eq!(
                expected,
                Some(history.journal.with_untracked(StudioJournal::recorded)),
                "a policy change records no undo step"
            );
        });
        owner.cleanup();
    }

    #[test]
    fn an_unknown_delete_becomes_the_entry_that_undoes_what_it_did() {
        let owner = Owner::new();
        owner.with(|| {
            let history = history(None, false, None, "11111111-1111-4111-8111-111111111111");
            let mut scene = two_zone_scene(9, 0.2);
            scene.run_epoch = "first".to_owned();
            let gone = DeletedZone::of(&scene.zones[1], 1, 1);
            let mut after = scene.clone();
            after.zones.remove(1);
            // The delete holds its place; an edit lands after it meanwhile.
            history.journal.update(|journal| {
                journal.record(StudioEdit::Unresolved {
                    token: gone.token,
                    name: gone.name.clone(),
                });
                journal.record(StudioEdit::Members(Vec::new()));
            });
            history.hold_unresolved(
                scene.id,
                gone,
                zones::Held {
                    release: Vec::new(),
                    fence: Fence::of(&scene),
                    key: "delete".to_owned(),
                },
            );
            assert!(history.has_unresolved());
            let item = history
                .unresolved
                .with_value(|held| held.first().cloned())
                .expect("held");
            // The daemon restarted, and the new run's scene has no zone.
            after.run_epoch = "next".to_owned();
            assert!(!settles_by_key(&item.held.fence.run, &after.run_epoch));
            let gone_now = !after
                .zones
                .iter()
                .any(|zone| zone.id.to_string() == item.record.zone_id);
            history.settle_held(&item, gone_now);
            history.journal.with_untracked(|journal| {
                assert_eq!(journal.cursor, 2, "settling neither adds nor moves a step");
                assert!(matches!(journal.entries[0], StudioEdit::ZoneDeleted(_)));
                assert!(matches!(journal.entries[1], StudioEdit::Members(_)));
            });
        });
        owner.cleanup();
    }

    #[test]
    fn an_operation_waits_for_pending_layout_commits_to_land() {
        let owner = Owner::new();
        owner.with(|| {
            let zone = "11111111-1111-4111-8111-111111111111";
            let history = history(None, false, None, zone);
            let pending = RwSignal::new(true);
            let drained = RwSignal::new(0_u32);
            history.attach_editor(Some(EditorHooks {
                commits_pending: Callback::new(move |()| pending.get_untracked()),
                drain_commits: Callback::new(move |()| drained.update(|count| *count += 1)),
                gesture_active: Callback::new(|()| false),
            }));
            history.delete_zone(zone.to_owned(), Callback::new(|_| {}));
            assert!(
                history.waiting.with_value(|waiting| waiting.len() == 1),
                "the delete waits for the commits"
            );
            assert!(drained.get_untracked() > 0, "and the writer is asked to go");
            // A scene released between two writes still starts nothing.
            history.resume();
            assert!(history.waiting.with_value(|waiting| waiting.len() == 1));
            // The writer goes idle and hands the scene back.
            pending.set(false);
            history.resume();
            assert!(history.waiting.with_value(VecDeque::is_empty));
        });
        owner.cleanup();
    }

    #[test]
    fn every_operation_waits_for_a_canvas_gesture_to_end() {
        let owner = Owner::new();
        owner.with(|| {
            let zone = "11111111-1111-4111-8111-111111111111";
            let history = history(None, false, None, zone);
            let dragging = RwSignal::new(true);
            history.attach_editor(Some(EditorHooks {
                commits_pending: Callback::new(|()| false),
                drain_commits: Callback::new(|()| {}),
                gesture_active: Callback::new(move |()| dragging.get_untracked()),
            }));
            history.delete_zone(zone.to_owned(), Callback::new(|_| {}));
            history.undo_deletion(1);
            assert!(
                history.waiting.with_value(|waiting| waiting.len() == 2),
                "a delete and a restore wait out the drag"
            );
            assert!(!history.busy.get_untracked(), "the canvas stays editable");
            dragging.set(false);
            history.resume();
            assert!(history.waiting.with_value(VecDeque::is_empty));
        });
        owner.cleanup();
    }

    #[test]
    fn a_waiting_undo_holds_for_a_gesture_and_skips_a_newer_edit() {
        let owner = Owner::new();
        owner.with(|| {
            let zone = "11111111-1111-4111-8111-111111111111";
            let history = history(None, false, None, zone);
            record_layout_edit(history, zone);
            let dragging = RwSignal::new(true);
            history.attach_editor(Some(EditorHooks {
                commits_pending: Callback::new(|()| false),
                drain_commits: Callback::new(|()| {}),
                gesture_active: Callback::new(move |()| dragging.get_untracked()),
            }));
            history.replay(false);
            assert!(
                history.waiting.with_value(|waiting| waiting.len() == 1),
                "the undo waits out the gesture"
            );
            // The gesture ends with a new edit of its own.
            record_layout_edit(history, zone);
            dragging.set(false);
            history.resume();
            assert!(history.waiting.with_value(VecDeque::is_empty));
            assert!(
                history.pending_layout.get_untracked().is_none(),
                "the stale undo stood down instead of undoing the new edit"
            );
            assert!(!history.busy.get_untracked());
        });
        owner.cleanup();
    }
}
