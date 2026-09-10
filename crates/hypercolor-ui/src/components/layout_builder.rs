//! The Studio Stage spatial layout editor.
//!
//! [`ZoneLayoutProvider`] owns the selected zone's editor session,
//! persistence, history, and live preview. [`LayoutWorkspace`] renders
//! that session inside the Stage.

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::app::{DevicesContext, WsContext};
use crate::components::layout_canvas::LayoutCanvas;
use crate::components::layout_palette::LayoutPalette;
use crate::components::layout_zone_properties::LayoutZoneProperties;
use crate::icons::*;
use crate::layout_geometry;
use crate::layout_history::{LayoutEditorSnapshot, RemovedOutputCache};
use crate::storage;
use crate::toasts;
use hypercolor_leptos_ext::events::target_is_text_entry;
use hypercolor_types::scene::ZoneRole;
use hypercolor_types::spatial::{EdgeBehavior, Output, SamplingMode, SpatialLayout};

// Panel size defaults and constraints
const SIDEBAR_DEFAULT: f64 = 280.0;
const SIDEBAR_MIN: f64 = 180.0;
const SIDEBAR_MAX: f64 = 480.0;
const BOTTOM_DEFAULT: f64 = 160.0;
const BOTTOM_MIN: f64 = 96.0;
const BOTTOM_MAX: f64 = 500.0;

const LS_KEY_SIDEBAR: &str = "hc-layout-sidebar-width";
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
    removed_zone_cache: ReadSignal<RemovedOutputCache>,
    set_removed_zone_cache: WriteSignal<RemovedOutputCache>,
    studio_history: crate::pages::studio::history::StudioHistory,
    selected_surface: Signal<Option<String>>,
    interaction_start: RwSignal<Option<LayoutEditorSnapshot>>,
    set_dirty: WriteSignal<bool>,
}

impl LayoutWriteHandle {
    fn capture_snapshot(self) -> Option<LayoutEditorSnapshot> {
        let current = self.layout.get_untracked()?;
        Some(LayoutEditorSnapshot {
            zones: current.zones,
            selected_zone_ids: self.selected_zone_ids.get_untracked(),
            compound_depth: self.compound_depth.get_untracked(),
            removed_zone_cache: self.removed_zone_cache.get_untracked(),
        })
    }

    pub(crate) fn apply_snapshot(self, snapshot: LayoutEditorSnapshot) {
        let LayoutEditorSnapshot {
            zones,
            selected_zone_ids,
            compound_depth,
            removed_zone_cache,
        } = snapshot;
        self.set_layout.update(move |current| {
            if let Some(layout) = current {
                layout.zones = zones;
            }
        });
        self.set_selected_zone_ids.set(selected_zone_ids);
        self.set_compound_depth.set(compound_depth);
        self.set_removed_zone_cache.set(removed_zone_cache);
    }

    pub fn in_interaction(self) -> bool {
        self.interaction_start.with_untracked(Option::is_some)
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
            self.studio_history.record_layout(zone_id, before, after);
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

    pub fn mark_clean(self) {
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
            self.studio_history.record_layout(zone_id, before, after);
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

    pub fn replace_zones_with_history(self, zones: Vec<Output>) {
        self.update(move |current| {
            if let Some(layout) = current {
                layout.zones = zones;
            }
        });
        self.set_dirty.set(true);
    }

    pub fn undo(self) {
        self.studio_history.replay(false);
    }

    pub fn redo(self) {
        self.studio_history.replay(true);
    }
}

mod editor_session;

pub(crate) use editor_session::{LayoutEditorContext, LayoutZoneDisplayContext};
use editor_session::{
    LayoutEditorSession, ZoneDraft, embedded_attachment_profiles, merge_draft, reconcile_replay,
};

/// The layout editor body with its device palette, canvas viewport,
/// zone-properties panel, and resizable-panel state. It consumes the
/// [`LayoutEditorContext`] provided by [`ZoneLayoutProvider`].
#[component]
pub(crate) fn LayoutWorkspace(
    /// Compact embedding (Studio Stage). The device palette collapses
    /// into a slide-over drawer instead of a permanent left column, so
    /// the canvas reads as the hero rather than one panel among four.
    #[prop(optional)]
    compact: bool,
) -> impl IntoView {
    let editor = expect_context::<LayoutEditorContext>();
    let has_layout = Signal::derive(move || editor.layout.with(Option::is_some));

    // Undo/redo shortcuts live in the workspace, not the provider: a
    // provider also wraps Studio's Screen and Unassigned Stages, where no
    // layout editor is shown. Keying them here scopes them to a visible
    // canvas.
    let can_undo = editor.can_undo;
    let can_redo = editor.can_redo;
    let write = editor.set_layout;
    let _history_shortcuts =
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

    // --- Resizable panel state ---
    let (sidebar_width, set_sidebar_width) = signal(load_panel_size(
        LS_KEY_SIDEBAR,
        SIDEBAR_DEFAULT,
        SIDEBAR_MIN,
        SIDEBAR_MAX,
    ));
    let (bottom_height, set_bottom_height) = signal(load_panel_size(
        LS_KEY_BOTTOM,
        BOTTOM_DEFAULT,
        BOTTOM_MIN,
        BOTTOM_MAX,
    ));

    // Which panel edge is being dragged (if any)
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum PanelDrag {
        Sidebar,
        Bottom,
    }
    let (dragging, set_dragging) = signal(None::<PanelDrag>);
    let container_ref = NodeRef::<leptos::html::Div>::new();

    // Global mousemove / mouseup listeners for drag (registered once)
    let _drag_move = window_event_listener(ev::mousemove, move |ev| {
        let Some(drag) = dragging.try_get_untracked().flatten() else {
            return;
        };
        let Some(container) = container_ref.try_get_untracked().flatten() else {
            return;
        };
        let rect = container.get_bounding_client_rect();

        match drag {
            PanelDrag::Sidebar => {
                let x = f64::from(ev.client_x()) - rect.left();
                let clamped = x.clamp(SIDEBAR_MIN, SIDEBAR_MAX.min(rect.width() - 200.0));
                set_sidebar_width.set(clamped);
            }
            PanelDrag::Bottom => {
                let y = f64::from(ev.client_y()) - rect.top();
                let panel_h = rect.height() - y;
                let clamped = panel_h.clamp(BOTTOM_MIN, BOTTOM_MAX.min(rect.height() - 120.0));
                set_bottom_height.set(clamped);
            }
        }
    });

    let _drag_end = window_event_listener(ev::mouseup, move |_| {
        let Some(drag) = dragging.try_get_untracked().flatten() else {
            return;
        };
        set_dragging.set(None);
        // Persist on release.
        match drag {
            PanelDrag::Sidebar => {
                if let Some(width) = sidebar_width.try_get_untracked() {
                    save_panel_size(LS_KEY_SIDEBAR, width);
                }
            }
            PanelDrag::Bottom => {
                if let Some(height) = bottom_height.try_get_untracked() {
                    save_panel_size(LS_KEY_BOTTOM, height);
                }
            }
        }
    });

    view! {
        <Show
            when=move || has_layout.get()
            fallback=move || {
                view! {
                    <div class="flex-1 flex items-center justify-center">
                        <div class="text-center space-y-3 animate-enter-fade">
                            <Icon icon=LuLayoutTemplate width="48px" height="48px"
                                  style="color: rgba(255, 106, 193, 0.25); filter: drop-shadow(0 0 12px rgba(255, 106, 193, 0.15))" />
                            <div class="text-fg-tertiary/50 text-sm">"Select or create a layout to begin"</div>
                            <div class="text-fg-tertiary/40 text-xs font-mono tracking-wide">"Drag devices onto the canvas to build your spatial mapping"</div>
                        </div>
                    </div>
                }
            }
        >
            <div
                class="relative flex min-h-0 flex-1 overflow-hidden"
                node_ref=container_ref
                style=move || {
                    match dragging.get() {
                        Some(PanelDrag::Sidebar) => "cursor: col-resize; user-select: none",
                        Some(PanelDrag::Bottom) => "cursor: row-resize; user-select: none",
                        None => "",
                    }
                }
            >
                // Full-page editor keeps the palette as a permanent
                // resizable column. Compact embeddings drop it here and
                // surface it through the slide-over drawer below instead.
                {(!compact).then(|| view! {
                    <div
                        class="shrink-0 min-h-0 overflow-y-auto"
                        style=move || format!("width: {:.0}px", sidebar_width.get())
                    >
                        <LayoutPalette />
                    </div>

                    <div
                        class="shrink-0 w-1 cursor-col-resize group/handle relative hover:bg-accent-muted/20
                               active:bg-accent-muted/30 transition-colors border-r border-edge-subtle"
                        on:mousedown=move |ev| {
                            ev.prevent_default();
                            set_dragging.set(Some(PanelDrag::Sidebar));
                        }
                    >
                        <div class="absolute inset-y-0 -left-0.5 -right-0.5" />
                        <div class="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-0.5 h-8
                                    rounded-full bg-fg-tertiary/20 group-hover/handle:bg-accent-muted/60 transition-colors" />
                    </div>
                })}

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
                        class="shrink-0 h-1 cursor-row-resize group/handle relative hover:bg-accent-muted/20
                               active:bg-accent-muted/30 transition-colors border-t border-edge-subtle"
                        on:mousedown=move |ev| {
                            ev.prevent_default();
                            set_dragging.set(Some(PanelDrag::Bottom));
                        }
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

/// The Studio Stage's zone-canvas actions — Save and Revert plus the
/// dirty and has-layout flags. Provided by [`ZoneLayoutProvider`]; the
/// Stage header consumes it. Undo / redo and the editor write handle come
/// from [`LayoutEditorContext`].
#[derive(Clone, Copy)]
pub(crate) struct ZoneCanvasActions {
    /// Persist the selected zone's layout through the per-zone API.
    pub save: Callback<()>,
    /// Restore the canvas to the last saved state.
    pub revert: Callback<()>,
    pub is_dirty: Signal<bool>,
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
/// persists them through the per-zone layout API at `PUT
/// .../zones/{id}/layout`. Switching zones switches the canvas. Mount it
/// once above the Stage; it provides
/// [`LayoutEditorContext`], [`LayoutZoneDisplayContext`], and
/// [`ZoneCanvasActions`].
#[component]
pub(crate) fn ZoneLayoutProvider(
    /// The selected zone's id (a `Zone` id). `None`, an unknown
    /// id, or a Display zone leaves the canvas empty.
    #[prop(into)]
    selected_zone_id: Signal<Option<String>>,
    /// Re-fetch the active scene after a save so the tree and Stage pick
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
            session
                .set_removed_zone_cache
                .set(RemovedOutputCache::new());
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
        if !dirty && let Some(preview) = active_preview_key.get_value() {
            ws_ctx.clear_zone_layout_preview.run(preview);
            active_preview_key.set_value(None);
        }
        if let Some(replay) = applied_replay {
            studio_history.complete_layout(replay.redo);
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

    // Every unsaved edit previews on the hardware, not just canvas drags:
    // a rotation, scale, align, or undo from the properties panel lands on
    // the LEDs the same way a drag does. Only a dirty layout pushes, so a
    // load, a revert, or a save never re-arms a preview.
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

    let save = Callback::new(move |()| {
        let Some(current) = layout.get_untracked() else {
            return;
        };
        let Some(zone_id) = selected_zone_id.get_untracked() else {
            return;
        };
        let Some(scene) = studio_history.current_scene() else {
            return;
        };
        let revision = scene.revision;
        let Some(generation) = studio_history.begin() else {
            return;
        };
        leptos::task::spawn_local(async move {
            match api::zones::update_zone_layout(&zone_id, &current, revision).await {
                Ok(api::zones::ZoneOutcome::Applied(zone)) => {
                    if !studio_history.is_current(generation) {
                        return;
                    }
                    let mut acknowledged = editor_layout_for_zone(&zone);
                    acknowledged.canvas_width = current.canvas_width;
                    acknowledged.canvas_height = current.canvas_height;
                    let acknowledged = layout_geometry::normalize_layout_for_editor(acknowledged);
                    drafts.update_value(|drafts| {
                        if let Some(draft) = drafts.get_mut(&(scene.id, zone_id.clone())) {
                            draft.baseline = acknowledged.clone();
                        }
                    });
                    studio_history.record_saved_zone(scene.id, zone, revision.saturating_add(1));
                    let completion = zone_save_completion(
                        selected_zone_id.get_untracked().as_deref(),
                        active_preview_key.get_value().as_deref(),
                        &zone_id,
                        studio_history.is_current(generation),
                    );
                    if completion.update_editor {
                        let clean = layout.with_untracked(|layout| {
                            layout
                                .as_ref()
                                .is_some_and(|layout| layout.zones == acknowledged.zones)
                        });
                        set_saved_layout.set(Some(acknowledged));
                        set_layout.set_dirty.set(!clean);
                        toasts::toast_success("Zone layout saved");
                    }
                    if completion.clear_preview {
                        ws_ctx.clear_zone_layout_preview.run(zone_id.clone());
                    }
                    if completion.clear_active_preview {
                        active_preview_key.set_value(None);
                    }
                    refresh_scene.run(());
                }
                Ok(api::zones::ZoneOutcome::Stale { .. }) => {
                    if !studio_history.is_current(generation) {
                        return;
                    }
                    let completion = zone_save_completion(
                        selected_zone_id.get_untracked().as_deref(),
                        active_preview_key.get_value().as_deref(),
                        &zone_id,
                        studio_history.is_current(generation),
                    );
                    if completion.clear_preview {
                        ws_ctx.clear_zone_layout_preview.run(zone_id.clone());
                    }
                    if completion.clear_active_preview {
                        active_preview_key.set_value(None);
                    }
                    if completion.update_editor {
                        toasts::toast_error("Scene changed elsewhere; reloaded, try again");
                    }
                    refresh_scene.run(());
                }
                Err(error) => {
                    if studio_history.is_current(generation) {
                        toasts::toast_error(&format!("Save failed: {error}"));
                    }
                }
            }
            studio_history.finish(generation);
        });
    });

    let revert = Callback::new(move |()| {
        if studio_history.busy.get_untracked() {
            return;
        }
        let Some(saved) = saved_layout.get_untracked() else {
            return;
        };
        set_layout.replace_zones_with_history(saved.zones.clone());
        set_layout.mark_clean();
        if let Some(key) = active_preview_key.get_value() {
            ws_ctx.clear_zone_layout_preview.run(key);
            active_preview_key.set_value(None);
        }
        toasts::toast_info("Zone layout reverted");
    });

    provide_context(ZoneCanvasActions {
        save,
        revert,
        is_dirty,
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
