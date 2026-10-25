//! One chronological journal for Studio placements and persisted assignments,
//! plus the multi-step zone delete and its undo.
//!
//! Every write is fenced by the scene revision, so one operation runs at a
//! time. `busy` marks a membership or zone operation or a layout replay in
//! flight, and locks the canvas; `committing` marks the layout commit queue
//! draining, which leaves editing alone. An operation asked for while
//! either runs waits its turn in order instead of being dropped.

use std::collections::{HashSet, VecDeque};

use super::placement::Footprint;
use crate::{api, layout_history::LayoutEditorSnapshot, toasts};
use hypercolor_types::api::scene::{EditMembersRequest, EditMembersResponse, MemberEdit};
use leptos::prelude::*;

#[derive(Clone, Debug, PartialEq)]
pub enum StudioEdit {
    Layout {
        zone_id: String,
        before: Box<LayoutEditorSnapshot>,
        after: Box<LayoutEditorSnapshot>,
    },
    Members(Vec<MemberEdit>),
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
    /// count when it was asked for.
    Replay {
        redo: bool,
        recorded: u64,
    },
    Assign {
        zone_id: String,
        device_id: String,
        segments: Vec<String>,
        placements: Vec<hypercolor_types::api::scene::MemberPlacementHint>,
    },
    Edit(Vec<MemberEdit>),
    DeleteZone {
        zone_id: String,
        select_after: Option<String>,
        deleted: Callback<DeletedZone>,
    },
    RestoreZone(DeletedZone),
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
    waiting: StoredValue<VecDeque<Waiting>>,
    editor: StoredValue<Option<EditorHooks>>,
    scene: Signal<Option<api::SceneDocument>>,
    latest: RwSignal<Option<api::SceneDocument>>,
    generation: RwSignal<u64>,
    selected: RwSignal<Option<String>>,
    refresh: Callback<()>,
}

impl StudioHistory {
    pub fn new(
        scene: Signal<Option<api::SceneDocument>>,
        selected: RwSignal<Option<String>>,
        refresh: Callback<()>,
    ) -> Self {
        let journal = RwSignal::new(StudioJournal::default());
        let busy = RwSignal::new(false);
        let committing = RwSignal::new(false);
        let waiting = StoredValue::new(VecDeque::new());
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
            waiting.set_value(VecDeque::new());
        });
        Self {
            journal,
            busy,
            committing,
            waiting,
            editor: StoredValue::new(None),
            pending_layout,
            scene,
            latest,
            generation,
            selected,
            refresh,
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
            } => self.assign_device_now(zone_id, device_id, segments, placements),
            Waiting::Edit(changes) => self.edit_members_now(changes),
            Waiting::DeleteZone {
                zone_id,
                select_after,
                deleted,
            } => self.delete_zone_now(zone_id, select_after, deleted),
            Waiting::RestoreZone(record) => self.restore_zone_now(record),
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

    pub fn assign_device(
        self,
        zone_id: String,
        device_id: String,
        segments: Vec<String>,
        placements: Vec<hypercolor_types::api::scene::MemberPlacementHint>,
    ) {
        self.when_free(Waiting::Assign {
            zone_id,
            device_id,
            segments,
            placements,
        });
    }

    fn assign_device_now(
        self,
        zone_id: String,
        device_id: String,
        segments: Vec<String>,
        placements: Vec<hypercolor_types::api::scene::MemberPlacementHint>,
    ) {
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
        self.submit(
            EditMembersRequest {
                scene_id: scene.id,
                changes: Vec::new(),
                assignment: Some(hypercolor_types::api::scene::MemberAssignmentTarget {
                    zone_id: zone.id,
                    device_id,
                    segments,
                    placements,
                }),
            },
            scene.revision,
        );
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
        self.edit_members(changes);
    }

    pub fn edit_members(self, changes: Vec<MemberEdit>) {
        if !changes.is_empty() {
            self.when_free(Waiting::Edit(changes));
        }
    }

    /// Send a membership edit against the scene as it stands now. Its
    /// preconditions are read afresh, since a layout commit may have moved
    /// an output while the edit waited; ownership and binding must hold.
    fn edit_members_now(self, changes: Vec<MemberEdit>) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Ok(changes) = rebase_preconditions(&scene, changes) else {
            toasts::toast_error("Assignment changed elsewhere; nothing was changed");
            return;
        };
        self.submit(
            EditMembersRequest {
                scene_id: scene.id,
                changes,
                assignment: None,
            },
            scene.revision,
        );
    }

    fn submit(self, request: EditMembersRequest, revision: u64) {
        let Some(generation) = self.begin() else {
            return;
        };
        leptos::task::spawn_local(async move {
            let result = api::zones::edit_members(&request, revision).await;
            // Disposed owners and superseded operations cannot consume a reply.
            if !self.is_current(generation) {
                return;
            }
            match result {
                Ok(api::zones::ZoneOutcome::Applied(response)) => {
                    if !response.changes.is_empty() {
                        self.journal
                            .update(|j| j.record(StudioEdit::Members(response.changes.clone())));
                    }
                    self.accept(response);
                    toasts::toast_success("Assignment saved");
                }
                Ok(api::zones::ZoneOutcome::Stale { .. }) => toasts::toast_error(
                    "Scene changed elsewhere; assignment was not saved. Try again.",
                ),
                Err(error) => toasts::toast_error(&format!("Assignment failed: {error}")),
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }

    fn accept(self, response: EditMembersResponse) {
        self.latest.set(Some(response.document));
    }

    pub fn replay(self, redo: bool) {
        let recorded = self.journal.with_untracked(StudioJournal::recorded);
        self.when_free(Waiting::Replay { redo, recorded });
    }

    fn replay_now(self, redo: bool, recorded: u64) {
        // An edit made while this waited is newer than what it was asked
        // to reverse; undoing that instead would surprise.
        if self.journal.with_untracked(StudioJournal::recorded) != recorded {
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

/// What a zone delete removed, kept so its undo can rebuild the zone.
#[derive(Clone, Debug, PartialEq)]
pub struct DeletedZone {
    pub name: String,
    pub color: Option<String>,
    pub enabled: bool,
    /// The zone's position in the scene's full zone order.
    pub index: usize,
    /// Complete snapshots of the outputs the zone held, in zone order.
    pub outputs: Vec<hypercolor_types::spatial::Output>,
}

/// The live scene's revision, read fresh between the steps of a
/// multi-request zone operation.
async fn live_revision() -> Option<u64> {
    api::fetch_active_scene()
        .await
        .ok()
        .map(|scene| scene.revision)
}

impl StudioHistory {
    /// Delete a zone. Its outputs leave first, through one membership
    /// transaction whose receipt holds their complete snapshots, so an
    /// undo can put them back exactly; then the empty zone goes.
    /// `select_after` takes the selection when the deleted zone held it.
    /// `deleted` runs with the restore record once the zone is gone.
    pub fn delete_zone(
        self,
        zone_id: String,
        select_after: Option<String>,
        deleted: Callback<DeletedZone>,
    ) {
        self.when_free(Waiting::DeleteZone {
            zone_id,
            select_after,
            deleted,
        });
    }

    fn delete_zone_now(
        self,
        zone_id: String,
        select_after: Option<String>,
        deleted: Callback<DeletedZone>,
    ) {
        use hypercolor_types::api::scene::MemberState;
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some((index, zone)) = scene
            .zones
            .iter()
            .enumerate()
            .find(|(_, zone)| zone.id.to_string() == zone_id)
        else {
            return;
        };
        let Some(generation) = self.begin() else {
            return;
        };
        let zone_key = zone.id;
        let changes: Vec<MemberEdit> = api::zone_outputs(zone)
            .into_iter()
            .enumerate()
            .map(|(index, output)| MemberEdit {
                before: Some(MemberState {
                    zone_id: zone_key,
                    output,
                    index,
                }),
                after: None,
            })
            .collect();
        let mut record = DeletedZone {
            name: zone.name.clone(),
            color: zone.color.clone(),
            enabled: zone.enabled,
            index,
            outputs: Vec::new(),
        };
        leptos::task::spawn_local(async move {
            let mut revision = scene.revision;
            if !changes.is_empty() {
                let request = EditMembersRequest {
                    scene_id: scene.id,
                    changes,
                    assignment: None,
                };
                match api::zones::edit_members(&request, revision).await {
                    Ok(api::zones::ZoneOutcome::Applied(response)) => {
                        revision = response.document.revision;
                        let mut released: Vec<_> = response
                            .changes
                            .into_iter()
                            .filter_map(|change| change.before)
                            .collect();
                        released.sort_by_key(|state| state.index);
                        record.outputs = released.into_iter().map(|state| state.output).collect();
                    }
                    Ok(api::zones::ZoneOutcome::Stale { .. }) => {
                        toasts::toast_error("Scene changed elsewhere. Reloaded, try again.");
                        self.refresh.run(());
                        self.finish(generation);
                        return;
                    }
                    Err(error) => {
                        toasts::toast_error(&format!("Zone delete failed: {error}"));
                        self.refresh.run(());
                        self.finish(generation);
                        return;
                    }
                }
            }
            match api::zones::delete_zone(&zone_id, revision).await {
                Ok(api::zones::ZoneOutcome::Applied(())) => {
                    if self.is_current(generation) {
                        if self.selected.get_untracked().as_deref() == Some(zone_id.as_str()) {
                            self.selected.set(select_after);
                        }
                        let _ = deleted.try_run(record);
                    }
                }
                outcome => {
                    // The outputs already left; put them back so a failed
                    // delete never strands the zone's devices.
                    if !record.outputs.is_empty()
                        && let Some(revision) = live_revision().await
                    {
                        let _ =
                            restore_outputs(scene.id, zone_key, &record.outputs, revision).await;
                    }
                    match outcome {
                        Err(error) => toasts::toast_error(&format!("Zone delete failed: {error}")),
                        _ => toasts::toast_error("Scene changed elsewhere. Reloaded, try again."),
                    }
                }
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }

    /// Rebuild a deleted zone: its name, color, and paused state, its
    /// outputs (those still unclaimed), and, best effort, its position.
    pub fn restore_zone(self, record: DeletedZone) {
        self.when_free(Waiting::RestoreZone(record));
    }

    fn restore_zone_now(self, record: DeletedZone) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(generation) = self.begin() else {
            return;
        };
        leptos::task::spawn_local(async move {
            let created =
                api::zones::create_zone(&record.name, record.color.as_deref(), scene.revision)
                    .await;
            let zone = match created {
                Ok(api::zones::ZoneOutcome::Applied(zone)) => zone,
                Ok(api::zones::ZoneOutcome::Stale { .. }) => {
                    toasts::toast_error("Scene changed elsewhere; the zone was not restored.");
                    self.refresh.run(());
                    self.finish(generation);
                    return;
                }
                Err(error) => {
                    toasts::toast_error(&format!("Zone restore failed: {error}"));
                    self.refresh.run(());
                    self.finish(generation);
                    return;
                }
            };
            let zone_id = zone.id;
            let mut problems = Vec::new();

            if let Ok(live) = api::fetch_active_scene().await {
                let claimed: HashSet<&str> = live
                    .zones
                    .iter()
                    .flat_map(|zone| zone.members.iter())
                    .map(|member| member.id.0.as_str())
                    .collect();
                let free: Vec<_> = record
                    .outputs
                    .iter()
                    .filter(|output| !claimed.contains(output.id.as_str()))
                    .cloned()
                    .collect();
                let moved_on = record.outputs.len() - free.len();
                if !free.is_empty()
                    && restore_outputs(live.id, zone_id, &free, live.revision)
                        .await
                        .is_err()
                {
                    problems.push("its devices could not be put back");
                } else if moved_on > 0 {
                    problems.push("devices you moved since stayed where they are");
                }
            }

            let zone_key = zone_id.to_string();
            if !record.enabled
                && !patch_zone(
                    &zone_key,
                    api::zones::PatchZoneRequest {
                        enabled: Some(false),
                        ..Default::default()
                    },
                )
                .await
            {
                problems.push("it could not be paused again");
            }
            let index = api::fetch_active_scene().await.ok().and_then(|live| {
                let current = live.zones.iter().position(|zone| zone.id == zone_id)?;
                (current != record.index).then_some(record.index)
            });
            if let Some(index) = index
                && !patch_zone(
                    &zone_key,
                    api::zones::PatchZoneRequest {
                        index: Some(index),
                        ..Default::default()
                    },
                )
                .await
            {
                problems.push("it kept its new place at the end of the tabs");
            }

            if self.is_current(generation) {
                self.selected.set(Some(zone_key));
            }
            if problems.is_empty() {
                toasts::toast_success(&format!("Restored {}", record.name));
            } else {
                toasts::toast_info(&format!(
                    "Restored {}, but {}",
                    record.name,
                    problems.join(" and ")
                ));
            }
            self.refresh.run(());
            self.finish(generation);
        });
    }
}

/// Put complete output snapshots back into a zone in one transaction.
async fn restore_outputs(
    scene_id: hypercolor_types::scene::SceneId,
    zone_id: hypercolor_types::scene::ZoneId,
    outputs: &[hypercolor_types::spatial::Output],
    revision: u64,
) -> Result<(), ()> {
    use hypercolor_types::api::scene::MemberState;
    let request = EditMembersRequest {
        scene_id,
        changes: outputs
            .iter()
            .enumerate()
            .map(|(index, output)| MemberEdit {
                before: None,
                after: Some(MemberState {
                    zone_id,
                    output: output.clone(),
                    index,
                }),
            })
            .collect(),
        assignment: None,
    };
    match api::zones::edit_members(&request, revision).await {
        Ok(api::zones::ZoneOutcome::Applied(_)) => Ok(()),
        _ => Err(()),
    }
}

/// Patch one zone against the live revision. Reports whether it applied.
async fn patch_zone(zone_id: &str, request: api::zones::PatchZoneRequest) -> bool {
    let Some(revision) = live_revision().await else {
        return false;
    };
    matches!(
        api::zones::update_zone(zone_id, &request, revision).await,
        Ok(api::zones::ZoneOutcome::Applied(_))
    )
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
            waiting: StoredValue::new(VecDeque::new()),
            editor: StoredValue::new(None),
            pending_layout: RwSignal::new(pending_layout),
            can_undo: Signal::stored(true),
            can_redo: Signal::stored(false),
            scene: Signal::stored(scene),
            latest: RwSignal::new(None),
            generation: RwSignal::new(1),
            selected: RwSignal::new(Some(selected.to_owned())),
            refresh: Callback::new(|()| {}),
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
