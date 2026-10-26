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
use crate::{api, layout_history::LayoutEditorSnapshot, toasts};
use hypercolor_types::api::scene::{
    EditMembersRequest, EditMembersResponse, MemberEdit, MemberPlacementHint, PatchZoneRequest,
};
use hypercolor_types::spatial::NormalizedPosition;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;

/// Runs once a membership write settles, with the scene revision it
/// landed at, or `None` when it did not apply. Every operation that takes
/// one settles it exactly once: when it lands, fails, stands down, or is
/// dropped from the queue. Only a Studio torn down mid-write drops it
/// unsettled, since nothing is left to hear it.
pub(crate) type EditDone = Box<dyn FnOnce(Option<u64>)>;

mod zones;

pub use zones::{DeletedZone, light_neighbor, restore_layer_requests, zone_patch_before};
pub(crate) use zones::{Moved, NewZone};

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
        select_after: Option<String>,
        deleted: Callback<DeletedZone>,
    },
    /// The Undo bar's restore of the deletion this token names.
    UndoDeletion(u64),
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
            | Self::UndoDeletion(_) => {}
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
        let scene_id = Memo::new(move |_| scene.with(|s| s.as_ref().map(|s| s.id)));
        Effect::new(move |_| {
            scene_id.get();
            journal.set(StudioJournal::default());
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
        Self {
            journal,
            busy,
            committing,
            waiting,
            tokens: StoredValue::new(0),
            editor: StoredValue::new(None),
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
        }
    }

    /// The active scene's id, tracked.
    pub fn scene_id(self) -> Signal<Option<hypercolor_types::scene::SceneId>> {
        let scene = self.scene;
        Signal::derive(move || scene.with(|scene| scene.as_ref().map(|scene| scene.id)))
    }

    pub fn current_scene(self) -> Option<api::SceneDocument> {
        let scene = self.scene.get_untracked()?;
        Some(
            self.latest
                .with_untracked(|latest| {
                    latest
                        .as_ref()
                        .filter(|s| s.id == scene.id && s.revision > scene.revision)
                        .cloned()
                })
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
                        .filter(|s| s.id == scene.id && s.revision > scene.revision)
                        .cloned()
                })
                .unwrap_or(scene),
        )
    }

    pub fn record_saved_zone(
        self,
        scene_id: hypercolor_types::scene::SceneId,
        zone: api::ZoneResource,
        revision: u64,
    ) {
        if let Some(mut scene) = self
            .current_scene()
            .filter(|scene| scene.id == scene_id && scene.revision <= revision)
        {
            if let Some(current) = scene.zones.iter_mut().find(|current| current.id == zone.id) {
                *current = zone;
            }
            scene.revision = revision;
            self.latest.set(Some(scene));
        }
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

    fn editor_hooks(self) -> Option<EditorHooks> {
        self.editor.try_get_value().flatten()
    }

    /// Whether `operation` must hold back for a canvas gesture.
    fn held_by_gesture(self, operation: &Waiting) -> bool {
        matches!(operation, Waiting::Replay { .. })
            && self
                .editor_hooks()
                .is_some_and(|editor| editor.gesture_active.run(()))
    }

    /// Run `operation` now when nothing holds the scene, otherwise after
    /// everything already waiting.
    fn when_free(self, operation: Waiting) {
        let queued = self
            .waiting
            .try_with_value(|waiting| !waiting.is_empty())
            .unwrap_or(false);
        if self.is_free() && !queued && !self.held_by_gesture(&operation) {
            self.run(operation);
        } else {
            self.waiting
                .try_update_value(|waiting| waiting.push_back(operation));
        }
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
            let editor = self.editor_hooks();
            if let Some(editor) = editor
                && editor.commits_pending.run(())
            {
                editor.drain_commits.run(());
                if !self.is_free() {
                    return;
                }
            }
            let held = self
                .waiting
                .try_with_value(|waiting| {
                    waiting
                        .front()
                        .is_some_and(|operation| self.held_by_gesture(operation))
                })
                .unwrap_or(false);
            if held {
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
            Waiting::DeleteZone {
                zone_id,
                select_after,
                deleted,
            } => self.delete_zone_now(zone_id, select_after, deleted),
            Waiting::UndoDeletion(token) => self.undo_deletion_now(token),
        }
    }

    pub fn complete_layout(self, redo: bool) {
        self.journal.update(|j| j.complete(redo));
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
        let newer = self
            .current_scene()
            .is_none_or(|current| current.id != scene.id || scene.revision > current.revision);
        if newer {
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
        self.submit(request, scene.revision, done);
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
        self.submit_with(request, scene.revision, Some(point), done);
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
            scene.revision,
            done,
        );
    }

    fn submit(self, request: EditMembersRequest, revision: u64, done: Option<EditDone>) {
        self.submit_with(request, revision, None, done);
    }

    /// Send a membership write as one operation and journal what landed.
    fn submit_with(
        self,
        request: EditMembersRequest,
        revision: u64,
        point: Option<NormalizedPosition>,
        done: Option<EditDone>,
    ) {
        let Some(generation) = self.begin() else {
            settle(done, None);
            return;
        };
        leptos::task::spawn_local(async move {
            let applied = match self
                .send_members(generation, request, revision, point)
                .await
            {
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
        revision: u64,
        point: Option<NormalizedPosition>,
    ) -> Sent {
        let scene_id = request.scene_id;
        let result = api::zones::edit_members(&request, revision).await;
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
            let result = api::zones::edit_members(&request, landed).await;
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
        self.latest.set(Some(response.document));
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
                    let result = api::zones::edit_members(&request, scene.revision).await;
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
            history.delete_zone(zone.to_owned(), None, Callback::new(|_| {}));
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
