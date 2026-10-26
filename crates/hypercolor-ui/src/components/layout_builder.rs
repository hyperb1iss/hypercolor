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

    /// Replace the editor's layout. A gesture under way ends with it, and
    /// Studio history picks up what waited for that gesture on the next
    /// tick.
    pub fn set(self, value: Option<SpatialLayout>) {
        let ended_gesture = self.interaction_start.with_untracked(Option::is_some);
        self.interaction_start.set(None);
        self.set_layout.set(value);
        self.set_dirty.set(false);
        if ended_gesture {
            let history = self.studio_history;
            leptos::task::spawn_local(async move { history.resume() });
        }
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
        self.close_interaction();
        self.studio_history.resume();
    }

    /// Close a gesture whose surface is being torn down. Studio history
    /// picks its queue up on the next tick, so a teardown during
    /// navigation never starts a write.
    pub fn abandon_interaction(self) {
        self.close_interaction();
        let history = self.studio_history;
        leptos::task::spawn_local(async move { history.resume() });
    }

    fn close_interaction(self) {
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

mod editor_session;
pub mod gesture;
pub mod ledger;
mod writer;

pub(crate) use editor_session::{LayoutEditorContext, LayoutZoneDisplayContext};
use editor_session::{LayoutEditorSession, embedded_attachment_profiles, reconcile_replay};

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
    /// Re-fetch the active scene after a commit so the device pane and
    /// Stage pick up the new scene revision.
    refresh_scene: Callback<()>,
    children: Children,
) -> impl IntoView {
    let devices_ctx = expect_context::<DevicesContext>();
    let ws_ctx = expect_context::<WsContext>();
    let render_canvas_size = crate::render_canvas::use_render_canvas_size();

    let studio_history = expect_context::<crate::pages::studio::history::StudioHistory>();
    let session = LayoutEditorSession::new(false, studio_history, selected_zone_id);
    let layout = session.layout;
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

    // The selected zone as the daemon last reported it, with the scene's
    // revision and the canvas it is drawn for. The zone's ledger turns it
    // into what the canvas shows.
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
        Some((
            scene.id,
            scene.revision,
            zone.clone(),
            render_canvas_size.get(),
        ))
    });
    // Moves whenever the writer changes a ledger, so the canvas redraws
    // what its zone now shows.
    let ledger_tick = RwSignal::new(0_u64);
    let loaded_key = StoredValue::new(None::<(hypercolor_types::scene::SceneId, String)>);
    Effect::new(move |_| {
        let next = canonical_zone.get();
        ledger_tick.track();
        let replay = studio_history.layout_replay_for_zone(
            next.as_ref()
                .map(|(_, _, zone, _)| zone.id.to_string())
                .as_deref(),
        );
        let Some((scene_id, revision, zone, canvas)) = next else {
            set_layout.set(None);
            loaded_key.set_value(None);
            return;
        };
        let zone_id = zone.id.to_string();
        writer::confirm(scene_id, &zone, revision, canvas);
        let Some(display) = writer::display(scene_id, &zone_id) else {
            return;
        };
        let mut shown = editor_layout_for_zone(&zone);
        shown.canvas_width = canvas.0;
        shown.canvas_height = canvas.1;
        shown.zones = display;
        let key = (scene_id, zone_id.clone());
        if loaded_key.get_value().as_ref() == Some(&key) {
            // The zone being edited, drawn again: its selection and any
            // gesture under way stay. What changed meanwhile joins the
            // gesture's press-time snapshot, so undoing the gesture takes
            // back the gesture alone; a field the snapshot holds at a value
            // of its own keeps it.
            let previous = layout
                .with_untracked(|current| current.as_ref().map(|layout| layout.zones.clone()));
            if let (Some(previous), Some(mut gesture)) =
                (previous, set_layout.interaction_start.get_untracked())
            {
                gesture::rebase_unedited(&mut gesture.zones, &previous, &shown.zones);
                set_layout.interaction_start.set(Some(gesture));
            }
            // A new revision that leaves the drawing alone (a layer tweak,
            // another zone's edit) touches nothing, so the canvas does not
            // redraw for it.
            let unchanged = layout.with_untracked(|current| {
                current.as_ref().is_some_and(|current| {
                    current.zones == shown.zones
                        && (current.canvas_width, current.canvas_height)
                            == (shown.canvas_width, shown.canvas_height)
                })
            });
            if !unchanged {
                let present: std::collections::HashSet<String> =
                    shown.zones.iter().map(|output| output.id.clone()).collect();
                set_layout.update_without_history(|current| *current = Some(shown));
                let gone = session
                    .selected_zone_ids
                    .with_untracked(|ids| ids.iter().any(|id| !present.contains(id)));
                if gone {
                    set_selected_zone_ids.update(|ids| ids.retain(|id| present.contains(id)));
                }
            }
        } else {
            set_layout.set(Some(shown));
            set_selected_zone_ids.set(std::collections::HashSet::new());
            set_compound_depth.set(crate::compound_selection::CompoundDepth::Root);
            loaded_key.set_value(Some(key));
        }
        let applied_replay = replay.filter(|replay| replay.zone_id == zone_id);
        if let Some(replay) = applied_replay.as_ref() {
            let mut snapshot = replay.snapshot.clone();
            let current = layout
                .get_untracked()
                .expect("selected editor was initialized");
            reconcile_replay(&current, &replay.previous, &mut snapshot);
            set_layout.apply_snapshot(snapshot);
            // Only the outputs the replay resized: normalizing is not always
            // a fixed point, and a box whose size did not change must not
            // read as a resize.
            set_layout.update_without_history(|layout| {
                let Some(layout) = layout.as_mut() else {
                    return;
                };
                let aspect =
                    layout_geometry::canvas_pixel_aspect(layout.canvas_width, layout.canvas_height);
                for output in &mut layout.zones {
                    let resized = current
                        .zones
                        .iter()
                        .find(|before| before.id == output.id)
                        .is_none_or(|before| before.size != output.size);
                    if resized {
                        output.size = layout_geometry::normalize_zone_size_for_editor(
                            output.size,
                            &output.topology,
                            output.shape.as_ref(),
                            aspect,
                        );
                    }
                }
            });
        }
        if let Some(replay) = applied_replay {
            // An undo or redo is an edit like any other: it makes intents,
            // ahead of anything that waited for it.
            set_layout.commit(zone_id.clone());
            studio_history.complete_layout(replay.redo);
        }
        let dirty = writer::pending(scene_id, &zone_id);
        set_layout.set_dirty.set(dirty);
        if !dirty
            && !set_layout.in_interaction()
            && let Some(preview) = active_preview_key.get_value()
        {
            ws_ctx.clear_zone_layout_preview.run(preview);
            active_preview_key.set_value(None);
        }
        // A newer scene lets a writer that paused on a failure go again.
        writer::wake();
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

    // The ledgers belong to the active scene, and another scene's never
    // land. Every zone with a ledger takes in each new scene, not only the
    // one on the canvas.
    Effect::new(move |_| {
        if let Some(scene) = studio_history.current_scene_tracked() {
            writer::confirm_known(&scene, render_canvas_size.get());
        }
    });

    // An edit becomes intents in the zone's ledger, and the writer sends
    // them.
    set_layout
        .on_commit
        .set_value(Some(Callback::new(move |zone_id: String| {
            let Some(edited) = layout
                .with_untracked(|current| current.as_ref().map(|layout| layout.zones.clone()))
            else {
                return;
            };
            let Some(scene_id) = studio_history.current_scene().map(|scene| scene.id) else {
                return;
            };
            if writer::edit(scene_id, &zone_id, &edited) {
                writer::wake();
            }
            set_layout
                .set_dirty
                .set(writer::pending(scene_id, &zone_id));
        })));

    // While this canvas is mounted, the writer takes its turn with Studio's
    // other scene operations, records what lands in Studio's history, and
    // redraws the canvas when a ledger changes.
    let lent = writer::attach(writer::WriterHooks {
        begin: Callback::new(move |()| studio_history.begin_commit()),
        end: Callback::new(move |generation| studio_history.end_commit(generation)),
        landed: Callback::new(
            move |(fence, zone, revision): (u64, api::ZoneResource, u64)| {
                if let Some(base) = studio_history
                    .current_scene()
                    .filter(|scene| scene.revision == fence)
                {
                    studio_history.record_saved_zone(&base, zone, revision);
                }
                refresh_scene.run(());
            },
        ),
        refetched: Callback::new(move |scene| studio_history.adopt(scene)),
        changed: Callback::new(move |_zone_id: String| ledger_tick.update(|tick| *tick += 1)),
        problem: Callback::new(move |message: String| toasts::toast_error(&message)),
        idle: Callback::new(move |()| studio_history.resume()),
    });
    on_cleanup(move || writer::detach(lent));

    // Studio history lets the writer go ahead of any operation waiting for
    // the scene, and holds a waiting undo or redo until a gesture ends.
    studio_history.attach_editor(Some(crate::pages::studio::history::EditorHooks {
        commits_pending: Callback::new(move |()| {
            studio_history
                .current_scene()
                .is_some_and(|scene| writer::busy(scene.id))
        }),
        drain_commits: Callback::new(move |()| writer::wake()),
        gesture_active: Callback::new(move |()| set_layout.in_interaction()),
    }));
    on_cleanup(move || studio_history.attach_editor(None));

    // An edit made while a structural operation held the scene goes out
    // once it finishes.
    Effect::new(move |_| {
        if !studio_history.busy.get() {
            writer::wake();
        }
    });

    // A writer that paused on a failure tries again once the socket opens
    // again.
    Effect::new(move |previous: Option<u64>| {
        let opened = ws_ctx.connection_generation.get();
        if previous.is_some_and(|previous| previous != opened) {
            writer::resume();
        }
        opened
    });

    // One edit: the whole group scales and recenters together, so undo
    // steps back over the Fit in one go.
    let fit = Callback::new(move |()| {
        // Frame the boxes the canvas shows; hidden and undrawn outputs move
        // with them.
        let hidden = session.hidden_zones.get_untracked();
        let framed: std::collections::HashSet<String> =
            devices_ctx.devices_resource.with_untracked(|devices| {
                let devices = devices
                    .as_ref()
                    .and_then(|result| result.as_ref().ok())
                    .map_or(&[][..], Vec::as_slice);
                layout.with_untracked(|current| {
                    current
                        .as_ref()
                        .map(|layout| {
                            let suppressed =
                                crate::layout_utils::suppressed_attachment_source_zone_ids(layout);
                            layout
                                .zones
                                .iter()
                                .filter(|zone| {
                                    !suppressed.contains(&zone.id)
                                        && !hidden.contains(&zone.id)
                                        && crate::components::layout_canvas::draws_output(
                                            zone, devices,
                                        )
                                })
                                .map(|zone| zone.id.clone())
                                .collect()
                        })
                        .unwrap_or_default()
                })
            });
        let mut fitted = false;
        set_layout.update(|current| {
            if let Some(layout) = current.as_mut() {
                let canvas = (layout.canvas_width, layout.canvas_height);
                fitted = crate::pages::studio::placement::fit_outputs(
                    &mut layout.zones,
                    canvas,
                    |output| framed.contains(&output.id),
                );
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
