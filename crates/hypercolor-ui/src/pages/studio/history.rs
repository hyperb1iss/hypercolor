//! One chronological journal for Studio placements and persisted assignments.

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
}

impl StudioJournal {
    pub fn record(&mut self, edit: StudioEdit) {
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

#[derive(Clone, Copy)]
pub(crate) struct StudioHistory {
    pub journal: RwSignal<StudioJournal>,
    pub busy: RwSignal<bool>,
    pub pending_layout: RwSignal<Option<LayoutReplay>>,
    pub can_undo: Signal<bool>,
    pub can_redo: Signal<bool>,
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
        });
        Self {
            journal,
            busy,
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

    pub fn begin(self) -> Option<u64> {
        if self.busy.get_untracked() {
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
        }
    }
    pub fn complete_layout(self, redo: bool) {
        self.journal.update(|j| j.complete(redo));
        self.pending_layout.set(None);
        self.busy.set(false);
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

    pub fn edit_members(self, changes: Vec<MemberEdit>) {
        if changes.is_empty() {
            return;
        }
        let Some(scene) = self.current_scene() else {
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
        let selected = self.selected.get_untracked();
        let destination = request.assignment.as_ref().and_then(|assignment| {
            (selected.as_deref() == Some(super::surface::UNASSIGNED_SURFACE_ID))
                .then(|| assignment.zone_id.to_string())
        });
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
                    // Keep the successful assignment and its Undo action visible,
                    // unless the user selected another surface while it saved.
                    if let Some(destination) = destination
                        && self.selected.get_untracked() == selected
                    {
                        self.selected.set(Some(destination));
                    }
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
        if self.busy.get_untracked() {
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

/// Resolve live preconditions while retaining unrelated placement edits.
/// Ownership and binding must still match the operation being reversed.
pub fn prepare_members_replay(
    scene: &api::SceneDocument,
    changes: Vec<MemberEdit>,
) -> Result<Vec<MemberEdit>, String> {
    changes
        .into_iter()
        .map(|mut change| {
            if let Some(before) = change.before.as_mut() {
                let zone = scene
                    .zones
                    .iter()
                    .find(|zone| zone.id == before.zone_id)
                    .ok_or("Zone no longer exists")?;
                let (index, output) = api::zone_outputs(zone)
                    .into_iter()
                    .enumerate()
                    .find(|(_, output)| output.id == before.output.id)
                    .ok_or("Output no longer belongs to the zone")?;
                if output.device_id != before.output.device_id
                    || output.zone_name != before.output.zone_name
                {
                    return Err("Output binding changed".into());
                }
                before.output = output.clone();
                before.index = index;
                if let Some(after) = change.after.as_mut() {
                    after.output = output;
                }
            }
            Ok(change)
        })
        .collect()
}

#[cfg(test)]
mod replay_tests {
    use super::*;

    #[test]
    fn changed_selection_cancels_pending_replay_without_consuming_history() {
        let owner = Owner::new();
        owner.with(|| {
            let snapshot = LayoutEditorSnapshot {
                zones: Vec::new(),
                selected_zone_ids: Default::default(),
                compound_depth: crate::compound_selection::CompoundDepth::Root,
                removed_zone_cache: Default::default(),
            };
            let journal = RwSignal::new(StudioJournal::default());
            journal.update(|journal| {
                journal.record(StudioEdit::Layout {
                    zone_id: "one".into(),
                    before: Box::new(snapshot.clone()),
                    after: Box::new(snapshot.clone()),
                })
            });
            let pending_layout = RwSignal::new(Some(LayoutReplay {
                zone_id: "one".into(),
                snapshot: snapshot.clone(),
                previous: snapshot,
                redo: false,
            }));
            let history = StudioHistory {
                journal,
                busy: RwSignal::new(true),
                pending_layout,
                can_undo: Signal::stored(true),
                can_redo: Signal::stored(false),
                scene: Signal::stored(None),
                latest: RwSignal::new(None),
                generation: RwSignal::new(1),
                selected: RwSignal::new(Some("two".into())),
                refresh: Callback::new(|()| {}),
            };
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
}
