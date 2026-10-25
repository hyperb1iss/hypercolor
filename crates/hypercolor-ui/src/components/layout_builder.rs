//! The Studio Stage spatial layout editor.
//!
//! [`ZoneLayoutProvider`] owns the selected zone's editor session, its
//! live commits, history, and live preview. [`LayoutWorkspace`] renders
//! that session inside the Stage. Layout edits commit when their gesture
//! ends, exactly like membership and layer edits; there is no staged save.

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::app::{DevicesContext, WsContext};
use crate::components::layout_canvas::LayoutCanvas;
use crate::components::layout_zone_properties::LayoutZoneProperties;
use crate::icons::*;
use crate::layout_geometry;
use crate::layout_history::LayoutEditorSnapshot;
use crate::pointer_gesture::{
    GestureEnd, PointerEnd, PointerGesture, Press, capture_pointer, holds_capture,
    suppress_press_defaults,
};
use crate::storage;
use crate::toasts;
use hypercolor_leptos_ext::events::target_is_text_entry;
use hypercolor_types::scene::ZoneRole;
use hypercolor_types::spatial::{EdgeBehavior, Output, SamplingMode, SpatialLayout};

// Zone-properties panel size default and constraints
const BOTTOM_DEFAULT: f64 = 160.0;
const BOTTOM_MIN: f64 = 96.0;
const BOTTOM_MAX: f64 = 500.0;

const LS_KEY_BOTTOM: &str = "hc-layout-bottom-height";

#[derive(Debug, PartialEq, Eq)]
struct ZoneSaveCompletion {
    update_editor: bool,
    clear_preview: bool,
    clear_active_preview: bool,
}

fn zone_save_completion(
    selected_zone_id: Option<&str>,
    active_preview_key: Option<&str>,
    saved_zone_id: &str,
    is_latest_save: bool,
) -> ZoneSaveCompletion {
    let active_preview_matches = active_preview_key == Some(saved_zone_id);
    ZoneSaveCompletion {
        update_editor: is_latest_save && selected_zone_id == Some(saved_zone_id),
        clear_preview: is_latest_save || !active_preview_matches,
        clear_active_preview: is_latest_save && active_preview_matches,
    }
}

fn load_panel_size(key: &str, default: f64, min: f64, max: f64) -> f64 {
    storage::get_clamped(key, default, min, max)
}

fn save_panel_size(key: &str, value: f64) {
    storage::set(key, &format!("{value:.0}"));
}

fn keyboard_target_is_text_input(target: Option<web_sys::EventTarget>) -> bool {
    target_is_text_entry(target)
}

#[derive(Clone, Copy)]
pub struct LayoutWriteHandle {
    layout: ReadSignal<Option<SpatialLayout>>,
    set_layout: WriteSignal<Option<SpatialLayout>>,
    selected_zone_ids: ReadSignal<std::collections::HashSet<String>>,
    set_selected_zone_ids: WriteSignal<std::collections::HashSet<String>>,
    compound_depth: ReadSignal<crate::compound_selection::CompoundDepth>,
    set_compound_depth: WriteSignal<crate::compound_selection::CompoundDepth>,
    studio_history: crate::pages::studio::history::StudioHistory,
    selected_surface: Signal<Option<String>>,
    interaction_start: RwSignal<Option<LayoutEditorSnapshot>>,
    set_dirty: WriteSignal<bool>,
    /// Commits a zone's edited layout; the provider installs it.
    on_commit: StoredValue<Option<Callback<String>>>,
}

impl LayoutWriteHandle {
    fn capture_snapshot(self) -> Option<LayoutEditorSnapshot> {
        let current = self.layout.get_untracked()?;
        Some(LayoutEditorSnapshot {
            zones: current.zones,
            selected_zone_ids: self.selected_zone_ids.get_untracked(),
            compound_depth: self.compound_depth.get_untracked(),
        })
    }

    pub(crate) fn apply_snapshot(self, snapshot: LayoutEditorSnapshot) {
        let LayoutEditorSnapshot {
            zones,
            selected_zone_ids,
            compound_depth,
        } = snapshot;
        self.set_layout.update(move |current| {
            if let Some(layout) = current {
                layout.zones = zones;
            }
        });
        self.set_selected_zone_ids.set(selected_zone_ids);
        self.set_compound_depth.set(compound_depth);
    }

    pub fn in_interaction(self) -> bool {
        self.interaction_start.with_untracked(Option::is_some)
    }

    /// Record a finished edit in Studio history and commit it.
    fn record(self, zone_id: String, before: LayoutEditorSnapshot, after: LayoutEditorSnapshot) {
        if before == after {
            return;
        }
        self.studio_history
            .record_layout(zone_id.clone(), before, after);
        self.commit(zone_id);
    }

    /// Commit the zone's current editor state.
    pub(crate) fn commit(self, zone_id: String) {
        if let Some(commit) = self.on_commit.get_value() {
            commit.run(zone_id);
        }
    }

    pub fn update(self, f: impl FnOnce(&mut Option<SpatialLayout>)) {
        if self.studio_history.busy.get_untracked() {
            return;
        }
        // Skip history bookkeeping while a drag/resize interaction is in flight —
        // begin_interaction already captured the pre-drag snapshot, and
        // finish_interaction will record the single combined diff on release.
        // Outside an interaction, capture before/after snapshots and record the edit.
        if self.in_interaction() {
            self.set_layout.update(f);
            return;
        }
        let before = self.capture_snapshot();
        self.set_layout.update(f);
        let (Some(before), Some(after)) = (before, self.capture_snapshot()) else {
            return;
        };
        if let Some(zone_id) = self.selected_surface.get_untracked() {
            self.record(zone_id, before, after);
        }
    }

    pub fn update_without_history(self, f: impl FnOnce(&mut Option<SpatialLayout>)) {
        self.set_layout.update(f);
    }

    pub fn set(self, value: Option<SpatialLayout>) {
        self.interaction_start.set(None);
        self.set_layout.set(value);
        self.set_dirty.set(false);
    }

    pub fn begin_interaction(self) {
        if self.studio_history.busy.get_untracked() {
            return;
        }
        if self.interaction_start.with_untracked(Option::is_none) {
            self.interaction_start.set(self.capture_snapshot());
        }
    }

    pub fn finish_interaction(self) {
        let before = self.interaction_start.get_untracked();
        self.interaction_start.set(None);
        if let (Some(before), Some(after), Some(zone_id)) = (
            before,
            self.capture_snapshot(),
            self.selected_surface.get_untracked(),
        ) {
            self.record(zone_id, before, after);
        }
    }

    /// Commit the in-flight drag/resize result in a single signal write.
    ///
    /// During drag the canvas paints positions directly to the DOM and never
    /// touches the layout signal, so this is the *only* moment the reactive
    /// graph sees the change. Returns true if zone state actually changed.
    pub fn commit_zones(self, zones: Vec<Output>) -> bool {
        if self.studio_history.busy.get_untracked() {
            return false;
        }
        let unchanged = self
            .layout
            .with_untracked(|l| l.as_ref().is_some_and(|current| current.zones == zones));
        if unchanged {
            return false;
        }
        self.set_layout.update(move |current| {
            if let Some(layout) = current {
                layout.zones = zones;
            }
        });
        self.set_dirty.set(true);
        true
    }

    pub fn undo(self) {
        self.studio_history.replay(false);
    }

    pub fn redo(self) {
        self.studio_history.replay(true);
    }
}

pub mod commit;
mod editor_session;

pub(crate) use editor_session::{LayoutEditorContext, LayoutZoneDisplayContext};
use editor_session::{
    LayoutEditorSession, ZoneDraft, embedded_attachment_profiles, merge_draft, reconcile_replay,
};

/// The layout editor body: the canvas viewport above a resizable
/// zone-properties panel. It consumes the [`LayoutEditorContext`]
/// provided by [`ZoneLayoutProvider`].
#[component]
pub(crate) fn LayoutWorkspace() -> impl IntoView {
    let editor = expect_context::<LayoutEditorContext>();
    let has_layout = Signal::derive(move || editor.layout.with(Option::is_some));

    // Undo/redo shortcuts live in the workspace, not the provider: a
    // provider also wraps Studio's Screen and Unassigned Stages, where no
    // layout editor is shown. Keying them here scopes them to a visible
    // canvas.
    let can_undo = editor.can_undo;
    let can_redo = editor.can_redo;
    let write = editor.set_layout;
    // Removed on cleanup: Leptos 0.8 leaves a window listener attached when
    // its handle drops, and a stale copy reads this workspace's disposed
    // signals on the next Ctrl+Z anywhere in the app.
    let history_shortcuts =
        window_event_listener(ev::keydown, move |ev: web_sys::KeyboardEvent| {
            if keyboard_target_is_text_input(ev.target()) {
                return;
            }
            if ev.alt_key() || !(ev.ctrl_key() || ev.meta_key()) {
                return;
            }
            // A drag in flight owns the layout until release; undoing under
            // it would be overwritten by the commit and leave history armed.
            if write.in_interaction() {
                return;
            }
            match ev.key().as_str() {
                "z" | "Z" if ev.shift_key() && can_redo.get_untracked() => {
                    ev.prevent_default();
                    write.redo();
                }
                "z" | "Z" if can_undo.get_untracked() => {
                    ev.prevent_default();
                    write.undo();
                }
                "y" | "Y" if can_redo.get_untracked() => {
                    ev.prevent_default();
                    write.redo();
                }
                _ => {}
            }
        });
    on_cleanup(move || history_shortcuts.remove());

    // --- Resizable zone-properties panel ---
    let (bottom_height, set_bottom_height) = signal(load_panel_size(
        LS_KEY_BOTTOM,
        BOTTOM_DEFAULT,
        BOTTOM_MIN,
        BOTTOM_MAX,
    ));

    // The splitter's height at the press, so a cancelled drag can put it
    // back.
    #[derive(Clone, Copy, Debug)]
    struct PanelGrip {
        start_size: f64,
    }
    let (dragging, set_dragging) = signal(false);
    let grip = StoredValue::new(PointerGesture::<PanelGrip>::new());
    let container_ref = NodeRef::<leptos::html::Div>::new();
    let bottom_grip_ref = NodeRef::<leptos::html::Div>::new();

    let grip_element =
        move || -> Option<web_sys::Element> { bottom_grip_ref.get_untracked().map(Into::into) };

    let finish_grip = move |outcome: GestureEnd, state: PanelGrip| {
        set_dragging.set(false);
        match outcome {
            GestureEnd::Cancel => set_bottom_height.set(state.start_size),
            // Persist on release.
            GestureEnd::Commit => {
                if let Some(height) = bottom_height.try_get_untracked() {
                    save_panel_size(LS_KEY_BOTTOM, height);
                }
            }
        }
    };

    // The splitter captures its pointer on press, so the drag keeps
    // tracking anywhere on screen without window listeners.
    let start_grip = move |ev: web_sys::PointerEvent| {
        suppress_press_defaults(&ev);
        let Some(element) = grip_element() else {
            return;
        };
        let pointer_id = ev.pointer_id();
        let press = grip
            .try_update_value(|g| {
                g.press(pointer_id, |owner, _| {
                    holds_capture(grip_element().as_ref(), owner)
                })
            })
            .unwrap_or(Press::Busy);
        match press {
            Press::Busy => return,
            Press::Stale(stale) => finish_grip(GestureEnd::Cancel, stale),
            Press::Ready => {}
        }
        let start_size = bottom_height.get_untracked();
        capture_pointer(&element, &ev);
        let button = ev.button();
        grip.update_value(|g| g.start(pointer_id, button, PanelGrip { start_size }));
        set_dragging.set(true);
    };

    let end_grip = move |ev: web_sys::PointerEvent, how: PointerEnd| {
        let ended = grip
            .try_update_value(|g| g.end(ev.pointer_id(), how))
            .flatten();
        if let Some((outcome, state)) = ended {
            finish_grip(outcome, state);
        }
    };

    let move_grip = move |ev: web_sys::PointerEvent| {
        if grip.with_value(|g| g.press_released(ev.pointer_id(), ev.buttons())) {
            end_grip(ev, PointerEnd::Up);
            return;
        }
        if !grip.with_value(|g| g.state(ev.pointer_id()).is_some()) {
            return;
        }
        let Some(container) = container_ref.try_get_untracked().flatten() else {
            return;
        };
        ev.prevent_default();
        let rect = container.get_bounding_client_rect();
        let y = f64::from(ev.client_y()) - rect.top();
        let panel_h = rect.height() - y;
        let clamped = panel_h.clamp(BOTTOM_MIN, BOTTOM_MAX.min(rect.height() - 120.0));
        set_bottom_height.set(clamped);
    };

    // The splitter renders under `Show`, so it can leave the DOM mid-drag
    // while this state lives on. A detached element's lost capture goes to
    // the document, not the splitter, so a window listener ends the drag.
    let detached_capture_loss = window_event_listener(ev::lostpointercapture, move |ev| {
        end_grip(ev, PointerEnd::LostCapture);
    });
    on_cleanup(move || detached_capture_loss.remove());

    view! {
        <Show
            when=move || has_layout.get()
            fallback=move || {
                view! {
                    <div class="flex-1 flex items-center justify-center">
                        <div class="text-center space-y-3 animate-enter-fade">
                            <Icon icon=LuLayoutTemplate width="48px" height="48px"
                                  style="color: rgba(255, 106, 193, 0.25); filter: drop-shadow(0 0 12px rgba(255, 106, 193, 0.15))" />
                            <div class="text-fg-tertiary/50 text-sm">"Choose a zone tab to edit its canvas"</div>
                        </div>
                    </div>
                }
            }
        >
            <div
                class="relative flex min-h-0 flex-1 overflow-hidden"
                node_ref=container_ref
                style=move || {
                    if dragging.get() {
                        "cursor: row-resize; user-select: none"
                    } else {
                        ""
                    }
                }
            >
                // Main area: canvas above, zone properties below
                <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
                    // Canvas viewport — flexes to fill remaining space.
                    // `isolate` traps the high per-box z-indexes in their own
                    // stacking context so device blocks never punch through an
                    // overlaid panel (the Studio composition slide-over).
                    <div class="relative isolate min-h-0 flex-1 overflow-hidden">
                        <LayoutCanvas />
                    </div>

                    // Bottom panel resize handle
                    <div
                        node_ref=bottom_grip_ref
                        class="shrink-0 h-1 cursor-row-resize group/handle relative touch-none touch-grab
                               hover:bg-accent-muted/20 active:bg-accent-muted/30 transition-colors border-t border-edge-subtle"
                        on:pointerdown=start_grip
                        on:pointermove=move_grip
                        on:pointerup=move |ev| end_grip(ev, PointerEnd::Up)
                        on:pointercancel=move |ev| end_grip(ev, PointerEnd::Cancel)
                        on:lostpointercapture=move |ev| end_grip(ev, PointerEnd::LostCapture)
                    >
                        <div class="absolute inset-x-0 -top-0.5 -bottom-0.5" />
                        <div class="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 h-0.5 w-8
                                    rounded-full bg-fg-tertiary/20 group-hover/handle:bg-accent-muted/60 transition-colors" />
                    </div>

                    // Zone properties — resizable height
                    <div
                        class="shrink-0 overflow-y-auto bg-surface-base/95 backdrop-blur-sm"
                        style=move || format!("height: {:.0}px", bottom_height.get())
                    >
                        <LayoutZoneProperties />
                    </div>
                </div>
            </div>
        </Show>
    }
}

/// The Studio Stage's zone-canvas actions: Fit, plus the flags the header
/// keys off. Provided by [`ZoneLayoutProvider`]; the Stage header consumes
/// it. Undo / redo and the editor write handle come from
/// [`LayoutEditorContext`].
#[derive(Clone, Copy)]
pub(crate) struct ZoneCanvasActions {
    /// Scale and center every member of the zone to fill its canvas.
    pub fit: Callback<()>,
    /// Whether the zone has members to fit.
    pub can_fit: Signal<bool>,
    /// Whether an editable zone layout is loaded. The header hides its
    /// actions when nothing is selected.
    pub has_layout: Signal<bool>,
}

/// What a queued commit would write against one scene document.
enum PreparedCommit {
    Write(SpatialLayout),
    Unchanged,
    Missing,
}

fn prepare_commit(scene: &api::SceneDocument, pending: &commit::PendingCommit) -> PreparedCommit {
    let Some(zone) = scene
        .zones
        .iter()
        .find(|zone| zone.id.to_string() == pending.zone_id)
    else {
        return PreparedCommit::Missing;
    };
    match commit::commit_placements(&api::zone_outputs(zone), &pending.baseline, &pending.edited) {
        Some(placements) => PreparedCommit::Write(SpatialLayout {
            zones: placements,
            ..editor_layout_for_zone(zone)
        }),
        None => PreparedCommit::Unchanged,
    }
}

fn editor_layout_for_zone(zone: &api::ZoneResource) -> SpatialLayout {
    SpatialLayout {
        id: zone.id.to_string(),
        name: zone.name.clone(),
        description: zone.description.clone(),
        canvas_width: 1,
        canvas_height: 1,
        zones: api::zone_outputs(zone),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

/// Sets up the editor signals, history, and live-preview wiring for the
/// Studio Stage, scoped to the selected zone's canonical placements.
///
/// This provider adapts the selected zone's normalized placements and
/// commits each finished edit through the per-zone layout API at `PUT
/// .../zones/{id}/layout`. Switching zones switches the canvas and never
/// drops an edit: its commit was queued when its gesture ended. Mount it
/// once above the Stage; it provides
/// [`LayoutEditorContext`], [`LayoutZoneDisplayContext`], and
/// [`ZoneCanvasActions`].
#[component]
pub(crate) fn ZoneLayoutProvider(
    /// The selected zone's id (a `Zone` id). `None`, an unknown
    /// id, or a Display zone leaves the canvas empty.
    #[prop(into)]
    selected_zone_id: Signal<Option<String>>,
    /// Re-fetch the active scene after a commit so the tree and Stage pick
    /// up the new scene revision.
    refresh_scene: Callback<()>,
    children: Children,
) -> impl IntoView {
    let devices_ctx = expect_context::<DevicesContext>();
    let ws_ctx = expect_context::<WsContext>();
    let render_canvas_size = crate::render_canvas::use_render_canvas_size();

    let studio_history = expect_context::<crate::pages::studio::history::StudioHistory>();
    let session = LayoutEditorSession::new(false, studio_history, selected_zone_id);
    let layout = session.layout;
    let saved_layout = session.saved_layout;
    let set_saved_layout = session.set_saved_layout;
    let set_selected_zone_ids = session.set_selected_zone_ids;
    let set_compound_depth = session.set_compound_depth;
    let set_layout = session.write;
    let is_dirty = session.is_dirty;

    // Previews are keyed by zone alone: the daemon applies them to the
    // live tree, so the scene is not the client's to choose.
    let active_preview_key = StoredValue::new(None::<String>);
    let push_preview = Callback::new(move |snapshot: SpatialLayout| {
        let Some(zone_id) = selected_zone_id.get_untracked() else {
            return;
        };
        active_preview_key.set_value(Some(zone_id.clone()));
        ws_ctx.send_zone_layout_preview.run((zone_id, snapshot));
    });

    Effect::new(move |_| {
        let next_key = selected_zone_id.get();
        let previous_key = active_preview_key.get_value();
        if previous_key != next_key {
            if let Some(key) = previous_key {
                ws_ctx.clear_zone_layout_preview.run(key);
            }
            active_preview_key.set_value(None);
        }
    });

    on_cleanup(move || {
        if let Some(key) = active_preview_key.get_value() {
            ws_ctx.clear_zone_layout_preview.run(key);
        }
    });

    session.provide_editor_context(push_preview);

    let attachment_profiles = embedded_attachment_profiles(devices_ctx.devices_resource);
    provide_context(LayoutZoneDisplayContext {
        attachment_profiles,
    });

    // Canonical placement changes refresh clean fields while local drafts keep
    // their edited fields. Layer control updates leave this memo unchanged.
    let canonical_zone = Memo::new(move |_| {
        let zone_id = selected_zone_id.get()?;
        let scene = studio_history.current_scene_tracked()?;
        let zone = scene
            .zones
            .iter()
            .find(|zone| zone.id.to_string() == zone_id)?;
        if zone.role == ZoneRole::Display {
            return None;
        }
        let mut canonical = editor_layout_for_zone(zone);
        let (width, height) = render_canvas_size.get();
        canonical.canvas_width = width;
        canonical.canvas_height = height;
        Some((
            scene.id,
            zone_id,
            layout_geometry::normalize_layout_for_editor(canonical),
        ))
    });
    let loaded_key = StoredValue::new(None::<(hypercolor_types::scene::SceneId, String)>);
    let drafts = StoredValue::new(std::collections::HashMap::<
        (hypercolor_types::scene::SceneId, String),
        ZoneDraft,
    >::new());
    Effect::new(move |_| {
        let next = canonical_zone.get();
        let replay = studio_history
            .layout_replay_for_zone(next.as_ref().map(|(_, zone_id, _)| zone_id.as_str()));
        // A commit landing mid-gesture reloads the same zone; the gesture
        // keeps its press-time snapshot so its release still records and
        // commits.
        let gesture = set_layout.interaction_start.get_untracked();
        // Capture the outgoing editor before changing the selected surface.
        if let (Some(key), Some(baseline), Some(snapshot)) = (
            loaded_key.get_value(),
            saved_layout.get_untracked(),
            set_layout.capture_snapshot(),
        ) {
            drafts.update_value(|drafts| {
                let mut next = ZoneDraft { baseline, snapshot };
                if let Some(previous) = drafts.get(&key) {
                    // Removed members can return through assignment Undo. Keep
                    // their drafts without rendering them while unassigned.
                    for output in &previous.snapshot.zones {
                        if !next
                            .snapshot
                            .zones
                            .iter()
                            .any(|current| current.id == output.id)
                        {
                            next.snapshot.zones.push(output.clone());
                        }
                    }
                    for output in &previous.baseline.zones {
                        if !next
                            .baseline
                            .zones
                            .iter()
                            .any(|current| current.id == output.id)
                        {
                            next.baseline.zones.push(output.clone());
                        }
                    }
                }
                drafts.insert(key, next);
            });
        }
        let Some((scene_id, zone_id, canonical)) = next else {
            set_layout.set(None);
            set_saved_layout.set(None);
            loaded_key.set_value(None);
            return;
        };
        let key = (scene_id, zone_id.clone());
        // Drafts from an old scene cannot be revived by a later scene switch.
        drafts.update_value(|drafts| drafts.retain(|(id, _), _| *id == scene_id));
        let draft = drafts.with_value(|drafts| drafts.get(&key).cloned());
        let snapshot = draft.as_ref().map(|draft| merge_draft(&canonical, draft));
        set_saved_layout.set(Some(canonical.clone()));
        set_layout.set(Some(canonical.clone()));
        if let Some(snapshot) = snapshot {
            set_layout.apply_snapshot(snapshot);
        } else {
            set_selected_zone_ids.set(std::collections::HashSet::new());
            set_compound_depth.set(crate::compound_selection::CompoundDepth::Root);
        }
        if loaded_key.get_value().as_ref() == Some(&key) && gesture.is_some() {
            set_layout.interaction_start.set(gesture);
        }
        loaded_key.set_value(Some(key));
        let applied_replay = replay.filter(|replay| replay.zone_id == zone_id);
        if let Some(replay) = applied_replay.as_ref() {
            let mut snapshot = replay.snapshot.clone();
            let current = layout
                .get_untracked()
                .expect("selected editor was initialized");
            reconcile_replay(&current, &replay.previous, &mut snapshot);
            set_layout.apply_snapshot(snapshot);
        }
        set_layout.update_without_history(|current| {
            if let Some(layout) = current.take() {
                *current = Some(layout_geometry::normalize_layout_for_editor(layout));
            }
        });
        let dirty = layout.with_untracked(|current| {
            current
                .as_ref()
                .is_some_and(|current| current.zones != canonical.zones)
        });
        set_layout.set_dirty.set(dirty);
        if !dirty
            && !set_layout.in_interaction()
            && let Some(preview) = active_preview_key.get_value()
        {
            ws_ctx.clear_zone_layout_preview.run(preview);
            active_preview_key.set_value(None);
        }
        if let Some(replay) = applied_replay {
            studio_history.complete_layout(replay.redo);
            // An undo or redo is an edit like any other: it commits.
            set_layout.commit(zone_id);
        }
    });

    // The daemon can retune its canvas live (and config lands after the
    // first scene paint), so the extent follows it without a reload. This
    // is not an edit: no history entry, no dirty flag.
    Effect::new(move |_| {
        let (canvas_width, canvas_height) = render_canvas_size.get();
        // Circular zones are normalized against the pixel aspect, so a live
        // extent change re-normalizes them for the new canvas too.
        let stamp = |layout: &mut Option<SpatialLayout>| {
            if let Some(layout) = layout.as_mut() {
                layout.canvas_width = canvas_width;
                layout.canvas_height = canvas_height;
                let aspect = layout_geometry::canvas_pixel_aspect(canvas_width, canvas_height);
                for zone in &mut layout.zones {
                    zone.size = layout_geometry::normalize_zone_size_for_editor(
                        zone.position,
                        zone.size,
                        &zone.topology,
                        zone.shape.as_ref(),
                        aspect,
                    );
                }
            }
        };
        let stale = layout.with_untracked(|current| {
            current
                .as_ref()
                .is_some_and(|l| (l.canvas_width, l.canvas_height) != (canvas_width, canvas_height))
        });
        if stale {
            set_layout.update_without_history(stamp);
            set_saved_layout.update(stamp);
        }
    });

    // Every edit previews on the hardware the moment it lands, not just
    // canvas drags: a rotation, scale, align, or undo from the properties
    // panel reaches the LEDs the same way a drag does, and its commit then
    // retires the preview. Only a layout that differs from the zone pushes,
    // so a load or a landed commit never re-arms a preview.
    Effect::new(move |previous: Option<Option<Vec<Output>>>| {
        let snapshot = layout.with(|current| current.as_ref().map(|l| l.zones.clone()));
        let changed = previous.is_some_and(|previous| previous != snapshot);
        if changed
            && is_dirty.get_untracked()
            && let Some(current) = layout.get_untracked()
        {
            push_preview.run(current);
        }
        snapshot
    });

    let commit_queue = StoredValue::new(commit::CommitQueue::default());
    // Edits queued against one scene cannot land in the next.
    let committed_scene = studio_history.scene_id();
    Effect::new(
        move |previous: Option<Option<hypercolor_types::scene::SceneId>>| {
            let current = committed_scene.get();
            if previous.is_some_and(|previous| previous != current) {
                commit_queue.set_value(commit::CommitQueue::default());
            }
            current
        },
    );

    // A landed commit becomes the editor's baseline, and its preview
    // retires unless newer edits for the zone are still queued.
    let acknowledge = move |scene_id: hypercolor_types::scene::SceneId,
                            zone_id: &str,
                            zone: api::ZoneResource,
                            revision: u64| {
        let (width, height) = render_canvas_size.get_untracked();
        let mut acknowledged = editor_layout_for_zone(&zone);
        acknowledged.canvas_width = width;
        acknowledged.canvas_height = height;
        let acknowledged = layout_geometry::normalize_layout_for_editor(acknowledged);
        drafts.update_value(|drafts| {
            if let Some(draft) = drafts.get_mut(&(scene_id, zone_id.to_owned())) {
                draft.baseline = acknowledged.clone();
            }
        });
        studio_history.record_saved_zone(scene_id, zone, revision);
        let completion = zone_save_completion(
            selected_zone_id.get_untracked().as_deref(),
            active_preview_key.get_value().as_deref(),
            zone_id,
            !commit_queue.with_value(|queue| queue.holds(zone_id)),
        );
        if completion.update_editor {
            let clean = layout.with_untracked(|layout| {
                layout
                    .as_ref()
                    .is_some_and(|layout| layout.zones == acknowledged.zones)
            });
            set_saved_layout.set(Some(acknowledged));
            set_layout.set_dirty.set(!clean);
        }
        if completion.clear_preview {
            ws_ctx.clear_zone_layout_preview.run(zone_id.to_owned());
        }
        if completion.clear_active_preview {
            active_preview_key.set_value(None);
        }
        refresh_scene.run(());
    };

    // Write one zone's edit. A stale revision means another client moved
    // the scene on: the edit is laid over the live scene and written once
    // more. An edit that still cannot land stays on the canvas, says so,
    // and rides along with the zone's next commit.
    let commit_zone = move |pending: commit::PendingCommit, generation: u64| async move {
        let Some(mut scene) = studio_history.current_scene() else {
            return;
        };
        let zone_id = pending.zone_id.clone();
        for rebased in [false, true] {
            let layout = match prepare_commit(&scene, &pending) {
                PreparedCommit::Write(layout) => layout,
                PreparedCommit::Unchanged => return,
                PreparedCommit::Missing => {
                    if studio_history.is_current(generation) {
                        toasts::toast_error("Layout change not saved: its zone no longer exists");
                    }
                    return;
                }
            };
            let revision = scene.revision;
            match api::zones::update_zone_layout(&zone_id, &layout, revision).await {
                Ok(api::zones::ZoneOutcome::Applied((zone, landed))) => {
                    if studio_history.is_current(generation) {
                        let landed = landed.unwrap_or_else(|| revision.saturating_add(1));
                        acknowledge(scene.id, &zone_id, zone, landed);
                    }
                    return;
                }
                Ok(api::zones::ZoneOutcome::Stale { .. }) if !rebased => {
                    let live = api::fetch_active_scene().await;
                    if !studio_history.is_current(generation) {
                        return;
                    }
                    match live {
                        Ok(live) if live.id == scene.id => scene = live,
                        _ => break,
                    }
                }
                Ok(api::zones::ZoneOutcome::Stale { .. }) => break,
                Err(error) => {
                    if studio_history.is_current(generation) {
                        toasts::toast_error(&format!("Layout change not saved: {error}"));
                    }
                    return;
                }
            }
        }
        if studio_history.is_current(generation) {
            toasts::toast_error(
                "Layout change not saved: the scene changed elsewhere. Your next edit retries it.",
            );
            refresh_scene.run(());
        }
    };

    // Writes are revision-fenced, so the queue drains one zone at a time
    // while editing carries on.
    let drain = move || {
        if commit_queue.with_value(commit::CommitQueue::is_empty) {
            return;
        }
        let Some(generation) = studio_history.begin_commit() else {
            return;
        };
        leptos::task::spawn_local(async move {
            while studio_history.is_current(generation) {
                let Some(pending) = commit_queue
                    .try_update_value(commit::CommitQueue::pop)
                    .flatten()
                else {
                    break;
                };
                commit_zone(pending, generation).await;
            }
            studio_history.end_commit(generation);
        });
    };

    set_layout
        .on_commit
        .set_value(Some(Callback::new(move |zone_id: String| {
            let Some(edited) = layout
                .with_untracked(|current| current.as_ref().map(|layout| layout.zones.clone()))
            else {
                return;
            };
            // The zone as the editor last loaded it: the edit is whatever
            // differs from here.
            let baseline = saved_layout
                .with_untracked(|saved| saved.as_ref().map(|saved| saved.zones.clone()))
                .unwrap_or_default();
            commit_queue.update_value(|queue| queue.push(zone_id, baseline, edited));
            drain();
        })));

    // A commit queued while a structural operation held the scene goes out
    // once it finishes.
    Effect::new(move |_| {
        if !studio_history.busy.get() {
            drain();
        }
    });

    // One edit: the whole group scales and recenters together, so undo
    // steps back over the Fit in one go.
    let fit = Callback::new(move |()| {
        let mut fitted = false;
        set_layout.update(|current| {
            if let Some(layout) = current.as_mut() {
                let canvas = (layout.canvas_width, layout.canvas_height);
                fitted = crate::pages::studio::placement::fit_outputs(&mut layout.zones, canvas);
            }
        });
        if fitted {
            set_layout.set_dirty.set(true);
        }
    });

    provide_context(ZoneCanvasActions {
        fit,
        can_fit: Signal::derive(move || {
            layout.with(|current| {
                current
                    .as_ref()
                    .is_some_and(|layout| !layout.zones.is_empty())
            })
        }),
        has_layout: Signal::derive(move || layout.with(Option::is_some)),
    });

    children()
}

#[cfg(test)]
mod zone_save_tests {
    use super::{ZoneSaveCompletion, zone_save_completion};

    #[test]
    fn current_zone_completion_updates_editor_and_retires_its_preview() {
        assert_eq!(
            zone_save_completion(Some("zone-a"), Some("zone-a"), "zone-a", true),
            ZoneSaveCompletion {
                update_editor: true,
                clear_preview: true,
                clear_active_preview: true,
            }
        );
    }

    #[test]
    fn old_zone_completion_does_not_mutate_the_new_zone() {
        assert_eq!(
            zone_save_completion(Some("zone-b"), Some("zone-b"), "zone-a", true),
            ZoneSaveCompletion {
                update_editor: false,
                clear_preview: true,
                clear_active_preview: false,
            }
        );
    }

    #[test]
    fn superseded_save_does_not_clear_the_newer_preview() {
        assert_eq!(
            zone_save_completion(Some("zone-a"), Some("zone-a"), "zone-a", false),
            ZoneSaveCompletion {
                update_editor: false,
                clear_preview: false,
                clear_active_preview: false,
            }
        );
    }
}
