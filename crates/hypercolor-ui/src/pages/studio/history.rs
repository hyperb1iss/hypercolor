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
pub(crate) use crate::api::Fence;
use crate::components::layout_builder::{keyed, writer};
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

mod announce;
mod held;
mod zones;

pub use announce::{DeviceLookup, ZoneLookup, replay_announcement};

pub use held::{members_landed, resolve_deletion, settles_by_key};
pub use zones::{
    DeletedZone, light_neighbor, rename_is_noop, restore_layer_requests, zone_patch_before,
};
pub(crate) use zones::{Moved, NewZone};

/// The scene Studio's history belongs to: the scene read now, or, while a
/// read has failed and none is known, the one it already belonged to.
#[must_use]
pub fn held_scene(read: Option<SceneId>, held: Option<SceneId>) -> Option<SceneId> {
    read.or(held)
}

/// The active scene as Studio holds it: the shared document, or, while a
/// read of it has failed, the last one read. A failed read is no news
/// about the scene, so a drag under way, the selection, and the history
/// all go on as they were.
#[must_use]
pub fn held_document(scene: Memo<Option<api::SceneDocument>>) -> Memo<Option<api::SceneDocument>> {
    Memo::new(move |previous: Option<&Option<api::SceneDocument>>| {
        scene.get().or_else(|| previous.cloned().flatten())
    })
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
    /// A step whose outcome a lost connection hid, `name` saying what it
    /// was. It holds its place in the journal until it settles, then
    /// becomes the entry that undoes or redoes what it did, or goes when
    /// that leaves nothing to undo or redo; until then neither undo nor
    /// redo moves past it.
    Unresolved {
        name: String,
    },
}

/// A journal entry's identity, unique to the page: it never changes while
/// the entry lives, however settling other entries moves it, and no other
/// journal (another scene's, or this scene's after a switch) ever reuses it.
pub type EntryId = u64;

/// The last entry identity handed out, for the life of the page.
static LAST_ENTRY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    id: EntryId,
    edit: StudioEdit,
    /// An undo or redo of it waits to settle, which may move the cursor.
    replaying: bool,
}

/// What a place in the journal holds once the step it stands for is done.
#[derive(Clone, Debug, PartialEq)]
pub enum Settled {
    /// The step left nothing to undo or redo: the place goes.
    Gone,
    /// The place holds this entry, applied: Undo takes it back.
    Applied(StudioEdit),
    /// The place holds this entry, not applied: Redo puts it back. Edits
    /// recorded after the place took the redo branch it would sit in, so
    /// with any of those still applied it goes instead.
    Unapplied(StudioEdit),
}

#[derive(Clone, Debug, Default)]
pub struct StudioJournal {
    entries: Vec<Entry>,
    cursor: usize,
    /// Edits recorded so far; replays do not count.
    recorded: u64,
}

impl StudioJournal {
    /// Record `edit` as the newest step, dropping the redo branch. Returns
    /// the identity of its place.
    pub fn record(&mut self, edit: StudioEdit) -> EntryId {
        self.recorded = self.recorded.wrapping_add(1);
        let id = LAST_ENTRY.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        self.entries.truncate(self.cursor);
        self.entries.push(Entry {
            id,
            edit,
            replaying: false,
        });
        if self.entries.len() > crate::layout_history::MAX_HISTORY_DEPTH {
            self.entries.remove(0);
        }
        self.cursor = self.entries.len();
        id
    }
    pub fn undo_edit(&self) -> Option<&StudioEdit> {
        self.cursor
            .checked_sub(1)
            .and_then(|i| self.entries.get(i))
            .map(|entry| &entry.edit)
    }
    pub fn redo_edit(&self) -> Option<&StudioEdit> {
        self.entries.get(self.cursor).map(|entry| &entry.edit)
    }
    /// The place an undo (`false`) or redo (`true`) would replay now, and
    /// its entry.
    pub fn replayed(&self, redo: bool) -> Option<(EntryId, &StudioEdit)> {
        let index = if redo {
            self.cursor
        } else {
            self.cursor.checked_sub(1)?
        };
        self.entries.get(index).map(|entry| (entry.id, &entry.edit))
    }
    /// The entry at the place `id`, while the journal holds it.
    pub fn edit(&self, id: EntryId) -> Option<&StudioEdit> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| &entry.edit)
    }
    /// Whether the place `id` is the newest step applied, the one an undo
    /// would replay.
    pub fn is_latest(&self, id: EntryId) -> bool {
        self.position(id)
            .is_some_and(|index| index + 1 == self.cursor)
    }
    /// The name of the step an undo (`false`) or redo (`true`) waits on
    /// now, while one does: the newest applied place, while it waits to
    /// settle. A redo waits only on a held undo or redo there, whose
    /// settling can move the cursor, so nothing replays across it.
    pub fn waiting_on(&self, redo: bool) -> Option<&str> {
        let entry = self.entries.get(self.cursor.checked_sub(1)?)?;
        match &entry.edit {
            StudioEdit::Unresolved { name } if !redo || entry.replaying => Some(name),
            _ => None,
        }
    }
    /// Changes whenever a new edit is recorded, so a waiting undo or redo
    /// can tell the history moved on after it was asked for.
    pub fn recorded(&self) -> u64 {
        self.recorded
    }
    fn position(&self, id: EntryId) -> Option<usize> {
        self.entries.iter().position(|entry| entry.id == id)
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
            match &mut entry.edit {
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

    /// Hold the place `id` while the step it stands for, which went out
    /// with no answer to say what it did, waits to settle. It shows as
    /// [`StudioEdit::Unresolved`] named `name`, among the applied steps
    /// (a redo's place moves the cursor past it), so a new edit never drops
    /// it with the redo branch. A journal that no longer holds the place is
    /// left alone.
    pub fn hold(&mut self, id: EntryId, name: String) {
        let Some(index) = self.position(id) else {
            return;
        };
        let entry = &mut self.entries[index];
        entry.edit = StudioEdit::Unresolved { name };
        entry.replaying = true;
        if index == self.cursor {
            self.cursor += 1;
        }
    }

    /// Settle the place `id` as `settled` (see [`Settled`]), wherever
    /// settling others moved it meanwhile: a step replayed from it just
    /// finished, or one held there settled. A journal that no longer holds
    /// it (a scene switch cleared it) is left alone.
    pub fn settle(&mut self, id: EntryId, settled: Settled) {
        let Some(index) = self.position(id) else {
            return;
        };
        match settled {
            Settled::Gone => self.remove(index),
            Settled::Applied(edit) => {
                self.entries[index] = Entry {
                    id,
                    edit,
                    replaying: false,
                };
                if index == self.cursor {
                    self.cursor += 1;
                }
            }
            Settled::Unapplied(edit) => {
                if index + 1 < self.cursor {
                    self.remove(index);
                } else {
                    self.entries[index] = Entry {
                        id,
                        edit,
                        replaying: false,
                    };
                    self.cursor = self.cursor.min(index);
                }
            }
        }
    }

    fn remove(&mut self, index: usize) {
        self.entries.remove(index);
        if index < self.cursor {
            self.cursor -= 1;
        }
    }

    /// The place of the deletion `token` names while it can still be
    /// undone.
    pub fn undoable_deletion(&self, token: u64) -> Option<EntryId> {
        self.entries[..self.cursor]
            .iter()
            .find(|entry| {
                matches!(&entry.edit, StudioEdit::ZoneDeleted(record) if record.token == token)
            })
            .map(|entry| entry.id)
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
    /// The rest of the undo or redo `item` held, now its first write is
    /// known to have landed: `did` is the entry that undoes what that
    /// write did.
    Resume {
        item: Box<held::Unresolved>,
        did: StudioEdit,
    },
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
            | Self::UnassignedBehavior(_)
            | Self::Resume { .. } => {}
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
    /// No answer came back, even under its key: what went out, to hold
    /// until it settles.
    Unknown(Box<held::Pending>),
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
    /// Writes whose outcome a lost connection hid, until they settle.
    unresolved: StoredValue<Vec<held::Unresolved>>,
    /// Moves each time a scene switch replaces the journal, so a settlement
    /// started for one journal never touches another's held writes.
    lifetime: StoredValue<u64>,
    /// Whether the held writes are being settled.
    settling: StoredValue<held::Settling>,
    scene: Signal<Option<api::SceneDocument>>,
    latest: RwSignal<Option<api::SceneDocument>>,
    /// The daemon run the socket last named, when there is a socket.
    run: Option<ReadSignal<Option<String>>>,
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
        let mut history = Self {
            journal,
            busy,
            committing,
            waiting,
            tokens: StoredValue::new(0),
            editor: StoredValue::new(None),
            announcer: StoredValue::new(None),
            replay_words: StoredValue::new(None),
            unresolved,
            lifetime: StoredValue::new(0),
            settling: StoredValue::new(held::Settling::Idle),
            pending_layout,
            scene,
            latest,
            run: None,
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
        let socket = use_context::<crate::app::WsContext>()
            .map(|ws| (ws.connection_generation, ws.run_epoch));
        history.run = socket.map(|(_, run)| run);
        // Another active scene starts a fresh journal.
        Effect::new(move |_| {
            scene_id.get();
            history.start_journal();
        });
        // A write no answer came back for settles once the connection is
        // back: sent again under its own key in its own daemon run, or read
        // from the scene once that run has ended.
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

    /// Start a fresh journal for a newly active scene: the old one's
    /// steps, held writes, and waiting operations go with it.
    fn start_journal(self) {
        self.journal.set(StudioJournal::default());
        self.unresolved.try_set_value(Vec::new());
        self.lifetime.try_update_value(|lifetime| *lifetime += 1);
        self.pending_layout.set(None);
        self.latest.set(None);
        self.generation.update(|g| *g = g.wrapping_add(1));
        self.busy.set(false);
        self.committing.set(false);
        let dropped = self
            .waiting
            .try_update_value(std::mem::take)
            .unwrap_or_default();
        for operation in dropped {
            operation.stand_down();
        }
    }

    /// The daemon run the socket last named.
    fn socket_run(self) -> Option<String> {
        self.run.and_then(|run| run.try_get_untracked().flatten())
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
                        .filter(|latest| newer(latest, &scene, self.socket_run().as_deref()))
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
                    let run = self.run.and_then(|run| run.get());
                    latest
                        .as_ref()
                        .filter(|latest| newer(latest, &scene, run.as_deref()))
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
            Waiting::Resume { item, did } => self.resume_now(*item, did),
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
                });
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
            let fence = Fence::of(&scene);
            let result = written(&fence, key.clone(), || {
                api::zones::update_unassigned_behavior(&behavior, &fence, &key)
            })
            .await;
            if self.is_current(generation) {
                match result {
                    Written::Answered(Ok(api::zones::ZoneOutcome::Applied(_))) => {}
                    Written::Answered(Ok(api::zones::ZoneOutcome::Stale { .. })) => {
                        toasts::toast_error("Scene changed elsewhere. Reloaded, try again.");
                    }
                    Written::Answered(Err(error)) => {
                        toasts::toast_error(&format!("Policy update failed: {error}"));
                    }
                    Written::Unknown(..) => toasts::toast_info(
                        "No answer came back for the unassigned-device policy, so it may not have been saved. Check the scene.",
                    ),
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
        let scene_id = request.scene_id;
        leptos::task::spawn_local(async move {
            let applied = match self.send_members(generation, request, fence, point).await {
                Sent::Superseded => {
                    self.settle_superseded(done);
                    return;
                }
                Sent::Failed => None,
                Sent::Unknown(pending) => {
                    self.record_held(scene_id, "the assignment".to_owned(), *pending);
                    None
                }
                Sent::Landed { changes, revision } => {
                    if !changes.is_empty() {
                        self.journal.update(|j| {
                            j.record(StudioEdit::Members(changes));
                        });
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
        let through = writer::edit_mark();
        let result = edit_members(&request, &fence).await;
        if !self.is_current(generation) {
            return Sent::Superseded;
        }
        let response = match result {
            Written::Answered(Ok(api::zones::ZoneOutcome::Applied(response))) => response,
            Written::Answered(outcome) => {
                report_failure(&outcome);
                if matches!(outcome, Ok(api::zones::ZoneOutcome::Stale { .. })) {
                    self.resync(generation).await;
                }
                return Sent::Failed;
            }
            Written::Unknown(fence, key) => {
                return Sent::Unknown(Box::new(held::Pending {
                    fence,
                    key,
                    write: held::HeldWrite::Members { request, through },
                }));
            }
        };
        let mut landed = response.document.revision;
        let mut changes = response.changes.clone();
        let canvas = self.canvas.get_untracked();
        let placed = point.map_or_else(Vec::new, |point| {
            center_arrivals(&response.changes, point, canvas)
        });
        self.accept(response, through);
        if !placed.is_empty() {
            let request = EditMembersRequest {
                scene_id,
                changes: placed,
                assignment: None,
            };
            let through = writer::edit_mark();
            let result = edit_members(&request, &fence.at(landed)).await;
            if !self.is_current(generation) {
                return Sent::Superseded;
            }
            match result {
                Written::Answered(Ok(api::zones::ZoneOutcome::Applied(response))) => {
                    landed = response.document.revision;
                    fold_placements(&mut changes, &response.changes);
                    self.accept(response, through);
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
        if let Ok(live) = api::fetch_active_scene_promptly().await
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

    /// Take in the receipt of a membership write that went out at the
    /// edit mark `through`.
    fn accept(self, response: EditMembersResponse, through: u64) {
        retire_placed(response.document.id, &response.changes, through);
        self.latest.set(Some(response.document));
    }

    /// Let go of the newest document history knew, as when the daemon may
    /// have started a new run whose revisions start over below it.
    pub fn forget_latest(self) {
        self.latest.try_set(None);
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
        if let Some(name) = self
            .journal
            .with_untracked(|j| j.waiting_on(redo).map(str::to_owned))
        {
            toasts::toast_info(&format!(
                "Still waiting to learn whether {name} went through. Undo and redo go on once the connection is back."
            ));
            return;
        }
        let replayed = self
            .journal
            .with_untracked(|j| j.replayed(redo).map(|(id, edit)| (id, edit.clone())));
        let Some((id, edit)) = replayed else {
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
                self.replay_zone_created(id, redo, zone_id, name, color, filled);
            }
            StudioEdit::ZonePatched {
                zone_id,
                before,
                after,
            } => {
                self.replay_zone_patched(id, redo, zone_id, before, after);
            }
            StudioEdit::ZoneDeleted(record) => {
                self.replay_zone_deleted_step(id, redo, *record);
            }
            // Only a held step's place shows unresolved, and that one waits
            // where undo would replay, which stopped this above.
            StudioEdit::Unresolved { .. } => {}
            StudioEdit::Members(changes) => self.replay_members(id, redo, changes),
        }
    }

    /// Undo or redo the membership edit at the journal place `id`.
    fn replay_members(self, id: EntryId, redo: bool, changes: Vec<MemberEdit>) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let entry = StudioEdit::Members(changes.clone());
        let changes = changes
            .into_iter()
            .map(|change| if redo { change } else { flip(change) })
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
            let through = writer::edit_mark();
            let result = edit_members(&request, &Fence::of(&scene)).await;
            if !self.is_current(generation) {
                return;
            }
            match result {
                Written::Answered(Ok(api::zones::ZoneOutcome::Applied(response))) => {
                    if self
                        .scene
                        .with_untracked(|s| s.as_ref().is_some_and(|s| s.id == scene.id))
                    {
                        let receipt = response.changes.clone();
                        let settled = if redo {
                            Settled::Applied(StudioEdit::Members(receipt))
                        } else {
                            Settled::Unapplied(StudioEdit::Members(
                                receipt.into_iter().map(flip).collect(),
                            ))
                        };
                        self.journal.update(|j| j.settle(id, settled));
                        self.announce_replay();
                        self.accept(response, through);
                        toasts::toast_success(if redo {
                            "Assignment redone and saved"
                        } else {
                            "Assignment undone and saved"
                        });
                    }
                }
                Written::Answered(Ok(api::zones::ZoneOutcome::Stale { .. })) => {
                    toasts::toast_error(
                        "Scene changed elsewhere; history was not applied. Try again.",
                    );
                }
                Written::Answered(Err(error)) => {
                    toasts::toast_error(&format!("History could not be applied: {error}"));
                }
                // Its place keeps the move until the write settles, so a
                // second Undo cannot send it again meanwhile.
                Written::Unknown(fence, key) => {
                    self.replay_words.try_set_value(None);
                    let what = if redo {
                        "redoing the assignment"
                    } else {
                        "undoing the assignment"
                    };
                    self.hold_replay(
                        scene.id,
                        id,
                        what.to_owned(),
                        held::Pending {
                            fence,
                            key,
                            write: held::HeldWrite::Members { request, through },
                        },
                        held::Then::Replay { entry, redo },
                    );
                }
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }
}

/// The same membership change, the other way round.
fn flip(change: MemberEdit) -> MemberEdit {
    MemberEdit {
        before: change.after,
        after: change.before,
    }
}

/// A membership write of ours that went out at the edit mark `through`
/// placed the outputs `changes` name: a layout edit made before it must not
/// land over it. Edits made since are newer, and still go out.
fn retire_placed(scene_id: SceneId, changes: &[MemberEdit], through: u64) {
    let placed: Vec<String> = changes
        .iter()
        .filter_map(|change| change.after.as_ref().or(change.before.as_ref()))
        .map(|state| state.output.id.clone())
        .collect();
    writer::forget(scene_id, &placed, through);
}

fn settle(done: Option<EditDone>, applied: Option<u64>) {
    if let Some(done) = done {
        done(applied);
    }
}

/// Whether `latest`, a document history adopted, is newer than the shared
/// `scene`. Revisions only compare within one daemon run, so across runs
/// the newer is the one of `live_run`, the run the socket names.
fn newer(latest: &api::SceneDocument, scene: &api::SceneDocument, live_run: Option<&str>) -> bool {
    if latest.id != scene.id {
        return false;
    }
    if latest.run_epoch == scene.run_epoch {
        return latest.revision > scene.revision;
    }
    live_run == Some(latest.run_epoch.as_str())
}

/// How a keyed write of history's came out.
pub(super) enum Written<T> {
    /// The daemon's final answer.
    Answered(api::ApiResult<api::zones::ZoneOutcome<T>>),
    /// No answer, even under its key, one from another daemon run, or one
    /// from a daemon that no longer knows the key's first answer: it may
    /// have landed or not. The fence and key it went out under, to settle
    /// it with once the connection is back.
    Unknown(Fence, String),
}

/// Send one write of history's fenced at `fence` under `key`, which
/// `attempt` sends it under, until the daemon answers.
pub(super) async fn written<T, F, Fut>(fence: &Fence, key: String, attempt: F) -> Written<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = api::client::KeyedResponse<T>>,
{
    match keyed::send(&fence.run, attempt).await {
        keyed::Answer::Final { outcome, .. } => Written::Answered(outcome),
        keyed::Answer::OtherRun | keyed::Answer::Forgotten | keyed::Answer::Lost(_) => {
            Written::Unknown(fence.clone(), key)
        }
    }
}

/// Send one membership edit fenced at `fence`, under an idempotency key of
/// its own that every retry of it reuses.
pub(super) async fn edit_members(
    request: &EditMembersRequest,
    fence: &Fence,
) -> Written<EditMembersResponse> {
    let key = keyed::new_key();
    written(fence, key.clone(), || {
        api::zones::edit_members(request, fence, &key)
    })
    .await
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
/// Untouched placement fields follow the live output; fields the edit
/// changed keep their requested values.
fn rebase_preconditions(
    scene: &api::SceneDocument,
    changes: Vec<MemberEdit>,
) -> Result<Vec<MemberEdit>, String> {
    changes
        .into_iter()
        .map(|mut change| {
            if let Some(before) = change.before.as_mut() {
                let (index, output) = live_member(scene, before)?;
                if let Some(after) = change.after.as_mut() {
                    for field in crate::components::layout_builder::ledger::Field::ALL {
                        if field.read(&after.output) == field.read(&before.output) {
                            field.write(&mut after.output, field.read(&output));
                        }
                    }
                }
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
            lifetime: StoredValue::new(1),
            settling: StoredValue::new(held::Settling::Idle),
            pending_layout: RwSignal::new(pending_layout),
            can_undo: Signal::stored(true),
            can_redo: Signal::stored(false),
            scene: Signal::stored(scene),
            latest: RwSignal::new(None),
            run: None,
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
            });
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
    fn a_queued_position_edit_keeps_remote_placement_fields() {
        use hypercolor_types::api::scene::MemberState;

        let owner = Owner::new();
        owner.with(|| {
            let scene = two_zone_scene(4, 0.2);
            let zone = &scene.zones[0];
            let before = MemberState {
                zone_id: zone.id,
                output: api::zone_outputs(zone).remove(0),
                index: 0,
            };
            let mut after = before.clone();
            after.output.position.x = 0.6;
            let history = history(Some(scene.clone()), false, None, &zone.id.to_string());
            history.committing.set(true);
            history.edit_members(
                vec![MemberEdit {
                    before: Some(before),
                    after: Some(after),
                }],
                None,
            );
            let Waiting::Edit { changes, .. } = history
                .waiting
                .try_update_value(VecDeque::pop_front)
                .flatten()
                .expect("the position edit waits for the commit")
            else {
                panic!("expected the queued position edit");
            };

            let mut live = scene;
            live.revision = 6;
            let placement = &mut live.zones[0]
                .layout
                .as_mut()
                .expect("zone layout")
                .placements[0];
            placement.position.y = 0.7;
            placement.size.x = 0.3;
            placement.size.y = 0.4;
            placement.rotation = 45.0;
            placement.scale = 1.5;
            placement.orientation = Some(hypercolor_types::spatial::Orientation::Vertical);
            let live_output = api::zone_outputs(&live.zones[0]).remove(0);
            let changes = rebase_preconditions(&live, changes).expect("the binding still holds");
            let before = &changes[0].before.as_ref().expect("precondition").output;
            let after = &changes[0].after.as_ref().expect("placement").output;
            assert_eq!(before, &live_output);
            assert_eq!(after.position.x, 0.6, "the requested position wins");
            assert_eq!(after.position.y, live_output.position.y);
            assert_eq!(after.size, live_output.size);
            assert_eq!(after.rotation, live_output.rotation);
            assert_eq!(after.scale, live_output.scale);
            assert_eq!(after.orientation, live_output.orientation);
        });
        owner.cleanup();
    }

    #[test]
    fn a_settled_membership_move_retires_only_the_layout_edits_made_before_it() {
        use hypercolor_types::api::scene::MemberState;
        let scene = two_zone_scene(4, 0.2);
        let zone = &scene.zones[1];
        let zone_id = zone.id.to_string();
        writer::observe(
            &scene,
            crate::render_canvas::DEFAULT_RENDER_CANVAS,
            Some(&zone_id),
        );
        let confirmed = writer::display(scene.id, &zone_id).expect("a confirmed zone");
        let mut edited = confirmed.clone();
        edited[0].position.x = 0.6;
        assert!(writer::edit(scene.id, &zone_id, &edited));
        // The move goes out here; the user drags again before it settles.
        let through = writer::edit_mark();
        edited[0].position.y = 0.3;
        assert!(writer::edit(scene.id, &zone_id, &edited));
        let keys = api::zone_outputs(zone).remove(0);
        let state = MemberState {
            zone_id: zone.id,
            output: keys,
            index: 0,
        };
        let changes = vec![MemberEdit {
            before: Some(state.clone()),
            after: Some(state),
        }];
        retire_placed(scene.id, &changes, through);
        let shown = writer::display(scene.id, &zone_id).expect("the zone");
        assert!(
            (shown[0].position.x - confirmed[0].position.x).abs() < f32::EPSILON,
            "the edit the move overtook retires"
        );
        assert!(
            (shown[0].position.y - 0.3).abs() < f32::EPSILON,
            "the edit made after the move stands"
        );
        assert!(writer::pending(scene.id, &zone_id));
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
            let name = format!("deleting {}", gone.name);
            let entry = history
                .journal
                .try_update(|journal| {
                    let entry = journal.record(StudioEdit::Unresolved { name: name.clone() });
                    journal.record(StudioEdit::Members(Vec::new()));
                    entry
                })
                .expect("a journal");
            history.hold(
                scene.id,
                entry,
                name.clone(),
                held::Pending {
                    fence: Fence::of(&scene),
                    key: "delete".to_owned(),
                    write: held::HeldWrite::Delete {
                        record: gone.clone(),
                        release: Vec::new(),
                    },
                },
                held::Then::Record,
            );
            assert!(history.has_unresolved());
            let item = history
                .unresolved
                .with_value(|held| held.first().cloned())
                .expect("held");
            // The daemon restarted, and the new run's scene has no zone.
            after.run_epoch = "next".to_owned();
            assert!(!settles_by_key(&item.pending.fence.run, &after.run_epoch));
            let gone_now = !after
                .zones
                .iter()
                .any(|zone| zone.id.to_string() == gone.zone_id);
            // Other operations claiming the scene meanwhile change nothing
            // about what is held.
            assert!(history.begin_commit().is_some());
            assert!(history.settle_if_held(&item, resolve_deletion(gone_now, &gone, &[])));
            assert!(!history.has_unresolved());
            history.journal.with_untracked(|journal| {
                assert_eq!(journal.cursor, 2, "settling neither adds nor moves a step");
                assert!(matches!(
                    journal.entries[0].edit,
                    StudioEdit::ZoneDeleted(_)
                ));
                assert!(matches!(journal.entries[1].edit, StudioEdit::Members(_)));
            });
            // A scene switch while a held write is out replaces the journal.
            // The old write's late answer settles nothing of the new scene's,
            // not even a write held where the old one was.
            let made = || StudioEdit::ZoneCreated {
                zone_id: gone.zone_id.clone(),
                name: gone.name.clone(),
                color: None,
                filled: Vec::new(),
            };
            let held_create = |name: &str| {
                let name = name.to_owned();
                let entry = history
                    .journal
                    .try_update(|journal| {
                        journal.record(StudioEdit::Unresolved { name: name.clone() })
                    })
                    .expect("a journal");
                history.hold(
                    scene.id,
                    entry,
                    name,
                    item.pending.clone(),
                    held::Then::Record,
                );
                entry
            };
            let old = held_create("creating Old");
            let stale = history
                .unresolved
                .with_value(|held| held.last().cloned())
                .expect("held");
            history.start_journal();
            let new = held_create("creating New");
            assert_ne!(new, old, "a new journal never reuses a place");
            let reused = held::Unresolved {
                entry: new,
                ..stale.clone()
            };
            assert!(!history.settle_if_held(&stale, Some(made())));
            assert!(!history.settle_if_held(&reused, Some(made())));
            assert_eq!(
                history.unresolved.with_value(Vec::len),
                1,
                "the new scene's write stays held"
            );
            history.journal.with_untracked(|journal| {
                assert_eq!(
                    journal.undo_edit(),
                    Some(&StudioEdit::Unresolved {
                        name: "creating New".to_owned()
                    })
                );
            });
            // A place the journal does not hold is never held.
            history.hold(
                scene.id,
                old,
                "creating Old".to_owned(),
                item.pending.clone(),
                held::Then::Record,
            );
            assert_eq!(history.unresolved.with_value(Vec::len), 1);
        });
        owner.cleanup();
    }

    /// A held write of `scene` for the settling tests; what it sent does
    /// not matter to how its place settles.
    fn held_item(
        scene: &api::SceneDocument,
        entry: EntryId,
        name: &str,
        then: held::Then,
    ) -> held::Unresolved {
        held::Unresolved {
            scene_id: scene.id,
            entry,
            name: name.to_owned(),
            pending: held::Pending {
                fence: Fence::of(scene),
                key: "key".to_owned(),
                write: held::HeldWrite::Members {
                    request: EditMembersRequest {
                        scene_id: scene.id,
                        changes: Vec::new(),
                        assignment: None,
                    },
                    through: 0,
                },
            },
            then,
            learned: None,
            lifetime: 1,
        }
    }

    #[test]
    fn a_held_undo_or_redo_settles_into_the_step_it_replays() {
        use hypercolor_types::api::scene::MemberState;
        let owner = Owner::new();
        owner.with(|| {
            let history = history(None, false, None, "11111111-1111-4111-8111-111111111111");
            let scene = two_zone_scene(9, 0.2);
            let output = api::zone_outputs(&scene.zones[0]).remove(0);
            let state = |zone: usize| MemberState {
                zone_id: scene.zones[zone].id,
                output: output.clone(),
                index: 0,
            };
            let moved = vec![MemberEdit {
                before: Some(state(0)),
                after: Some(state(1)),
            }];
            let entry = StudioEdit::Members(moved.clone());

            // An undo that landed: its receipt is the move back, and the
            // place keeps the move, ready to redo.
            let id = history
                .journal
                .try_update(|journal| journal.record(entry.clone()))
                .expect("a journal");
            history
                .journal
                .update(|journal| journal.hold(id, "undoing the assignment".to_owned()));
            let undo = held_item(
                &scene,
                id,
                "undoing the assignment",
                held::Then::Replay {
                    entry: entry.clone(),
                    redo: false,
                },
            );
            let receipt = moved.iter().cloned().map(flip).collect();
            history.settle_held(&undo, Some(StudioEdit::Members(receipt)));
            history.journal.with_untracked(|journal| {
                assert_eq!(journal.undo_edit(), None);
                assert_eq!(journal.redo_edit(), Some(&entry));
            });

            // A redo that did nothing goes back where it was.
            let id = history
                .journal
                .with_untracked(|journal| journal.replayed(true).map(|(id, _)| id))
                .expect("a step to redo");
            history
                .journal
                .update(|journal| journal.hold(id, "redoing the assignment".to_owned()));
            let redo = held_item(
                &scene,
                id,
                "redoing the assignment",
                held::Then::Replay {
                    entry: entry.clone(),
                    redo: true,
                },
            );
            history.settle_held(&redo, None);
            history.journal.with_untracked(|journal| {
                assert_eq!(journal.undo_edit(), None);
                assert_eq!(journal.redo_edit(), Some(&entry));
            });

            // A move into a zone a redo made again keeps what it moved.
            let made = scene.zones[1].id.to_string();
            let id = history
                .journal
                .try_update(|journal| journal.record(StudioEdit::Unresolved { name: "m".into() }))
                .expect("a journal");
            let fill = held_item(
                &scene,
                id,
                "m",
                held::Then::Fill {
                    zone_id: made.clone(),
                    name: "Keyboard".to_owned(),
                    color: None,
                },
            );
            history.settle_held(&fill, Some(StudioEdit::Members(moved.clone())));
            history.journal.with_untracked(|journal| {
                assert_eq!(
                    journal.undo_edit(),
                    Some(&StudioEdit::ZoneCreated {
                        zone_id: made.clone(),
                        name: "Keyboard".to_owned(),
                        color: None,
                        filled: moved.clone(),
                    })
                );
            });

            // A restore whose create did not land leaves the deletion as it
            // was; one that landed goes on in its turn, the place waiting.
            let record = DeletedZone::of(&scene.zones[1], 1, 4);
            let resume = || {
                held::Then::Resume(Box::new(held::Resume::Rebuild {
                    record: record.clone(),
                    remove: false,
                }))
            };
            for landed in [false, true] {
                let id = history
                    .journal
                    .try_update(|journal| {
                        journal.record(StudioEdit::ZoneDeleted(Box::new(record.clone())))
                    })
                    .expect("a journal");
                history
                    .journal
                    .update(|journal| journal.hold(id, "restoring Keyboard".to_owned()));
                let item = held_item(&scene, id, "restoring Keyboard", resume());
                // Another operation holds the scene, so the rest waits.
                let generation = history.begin().expect("a free scene");
                let made = StudioEdit::ZoneCreated {
                    zone_id: hypercolor_types::scene::ZoneId::new().to_string(),
                    name: "Keyboard".to_owned(),
                    color: None,
                    filled: Vec::new(),
                };
                history.settle_held(&item, landed.then_some(made));
                let waiting = history.waiting.with_value(|waiting| {
                    waiting
                        .iter()
                        .filter(|operation| matches!(operation, Waiting::Resume { .. }))
                        .count()
                });
                history.journal.with_untracked(|journal| {
                    if landed {
                        assert_eq!(waiting, 1, "the rest of the restore waits its turn");
                        assert_eq!(journal.waiting_on(false), Some("restoring Keyboard"));
                    } else {
                        assert_eq!(waiting, 0);
                        assert_eq!(
                            journal.undo_edit(),
                            Some(&StudioEdit::ZoneDeleted(Box::new(record.clone())))
                        );
                    }
                });
                history.finish(generation);
                history.waiting.set_value(VecDeque::new());
            }
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
