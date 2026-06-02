//! The spatial layout editor.
//!
//! `LayoutBuilder` is a thin shell: it mounts `LayoutEditorProvider`
//! (all editor state, persistence, and history wiring), its own
//! `PageHeader`, and the headless `LayoutWorkspace` body.
//!
//! The provider and workspace are split apart so the Studio Stage can
//! compose its *own* header around the same editor — it mounts a
//! `LayoutEditorProvider` on an ancestor, reads the lifted
//! [`LayoutEditorState`] from context, and renders a bare
//! `LayoutWorkspace`. Editing `/layout` and editing inside Studio drive
//! one shared editor; only the header chrome differs.
//!
//! Edits are pushed to the spatial engine immediately for live preview.
//! Save persists to disk. Revert restores to the last saved state.

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::use_debounce_fn_with_arg;

use crate::api;
use crate::app::{DevicesContext, WsContext};
use crate::components::control_panel::ControlDropdownDismissHandlers;
use crate::components::layout_canvas::LayoutCanvas;
use crate::components::layout_palette::LayoutPalette;
use crate::components::layout_zone_properties::LayoutZoneProperties;
use crate::components::page_header::{HeaderToolbar, HeaderTrailing, PageAccent, PageHeader};
use crate::components::silk_select::SilkSelect;
use crate::icons::*;
use crate::layout_geometry;
use crate::layout_history::{LayoutEditorSnapshot, LayoutHistoryState};
use crate::layout_page_state::{LayoutPageState, PerLayoutState};
use crate::storage;
use crate::toasts;
use hypercolor_leptos_ext::events::{Input, target_is_text_entry};
use hypercolor_types::scene::ZoneRole;
use hypercolor_types::spatial::{Output, SpatialLayout};

// Panel size defaults and constraints
const SIDEBAR_DEFAULT: f64 = 280.0;
const SIDEBAR_MIN: f64 = 180.0;
const SIDEBAR_MAX: f64 = 480.0;
const BOTTOM_DEFAULT: f64 = 160.0;
const BOTTOM_MIN: f64 = 96.0;
const BOTTOM_MAX: f64 = 500.0;

const LS_KEY_SIDEBAR: &str = "hc-layout-sidebar-width";
const LS_KEY_BOTTOM: &str = "hc-layout-bottom-height";

fn load_panel_size(key: &str, default: f64, min: f64, max: f64) -> f64 {
    storage::get_clamped(key, default, min, max)
}

fn save_panel_size(key: &str, value: f64) {
    storage::set(key, &format!("{value:.0}"));
}

fn preferred_replacement_layout(
    layouts: &[api::LayoutSummary],
    removed_layout_id: &str,
) -> Option<api::LayoutSummary> {
    layouts
        .iter()
        .find(|layout| layout.id != removed_layout_id && layout.is_active)
        .cloned()
        .or_else(|| {
            layouts
                .iter()
                .find(|layout| layout.id != removed_layout_id)
                .cloned()
        })
}

fn replacement_layout_name(layouts: &[api::LayoutSummary]) -> String {
    let base = "Default Layout";
    let existing_names: Vec<&str> = layouts.iter().map(|layout| layout.name.as_str()).collect();
    if !existing_names
        .iter()
        .any(|name| name.eq_ignore_ascii_case(base))
    {
        return base.to_owned();
    }

    let mut suffix = 2_u32;
    loop {
        let candidate = format!("{base} {suffix}");
        if !existing_names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

fn keyboard_target_is_text_input(target: Option<web_sys::EventTarget>) -> bool {
    target_is_text_entry(target)
}

#[derive(Clone, Copy)]
pub(crate) struct LayoutWriteHandle {
    layout: ReadSignal<Option<SpatialLayout>>,
    set_layout: WriteSignal<Option<SpatialLayout>>,
    selected_zone_ids: ReadSignal<std::collections::HashSet<String>>,
    set_selected_zone_ids: WriteSignal<std::collections::HashSet<String>>,
    compound_depth: ReadSignal<crate::compound_selection::CompoundDepth>,
    set_compound_depth: WriteSignal<crate::compound_selection::CompoundDepth>,
    removed_zone_cache: ReadSignal<crate::layout_utils::ZoneCache>,
    set_removed_zone_cache: WriteSignal<crate::layout_utils::ZoneCache>,
    history: RwSignal<LayoutHistoryState>,
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

    fn apply_snapshot(self, snapshot: LayoutEditorSnapshot) {
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

    fn in_interaction(self) -> bool {
        self.history
            .with_untracked(LayoutHistoryState::is_interactive)
    }

    pub fn update(self, f: impl FnOnce(&mut Option<SpatialLayout>)) {
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
        self.history
            .update(|state| state.record_edit(before, &after));
    }

    pub fn update_without_history(self, f: impl FnOnce(&mut Option<SpatialLayout>)) {
        self.set_layout.update(f);
    }

    pub fn set(self, value: Option<SpatialLayout>) {
        self.history.update(LayoutHistoryState::discard_interaction);
        self.set_layout.set(value);
        self.set_dirty.set(false);
    }

    pub fn mark_clean(self) {
        self.set_dirty.set(false);
    }

    pub fn reset_history(self) {
        self.history.update(LayoutHistoryState::reset);
    }

    pub fn begin_interaction(self) {
        if let Some(snapshot) = self.capture_snapshot() {
            self.history
                .update(|state| state.begin_interaction(snapshot));
        }
    }

    pub fn finish_interaction(self) {
        if let Some(current) = self.capture_snapshot() {
            self.history
                .update(|state| state.finish_interaction(&current));
        } else {
            self.history.update(LayoutHistoryState::discard_interaction);
        }
    }

    /// Commit the in-flight drag/resize result in a single signal write.
    ///
    /// During drag the canvas paints positions directly to the DOM and never
    /// touches the layout signal, so this is the *only* moment the reactive
    /// graph sees the change. Returns true if zone state actually changed.
    pub fn commit_zones(self, zones: Vec<Output>) -> bool {
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
        let Some(current) = self.capture_snapshot() else {
            return;
        };
        let mut restored = None;
        self.history.update(|state| {
            restored = state.undo(current.clone());
        });
        if let Some(snapshot) = restored {
            self.apply_snapshot(snapshot);
            self.set_dirty.set(true);
        }
    }

    pub fn redo(self) {
        let Some(current) = self.capture_snapshot() else {
            return;
        };
        let mut restored = None;
        self.history.update(|state| {
            restored = state.redo(current.clone());
        });
        if let Some(snapshot) = restored {
            self.apply_snapshot(snapshot);
            self.set_dirty.set(true);
        }
    }
}

/// Shared layout editor state — provided via context to palette, canvas, and zone properties.
#[derive(Clone, Copy)]
pub(crate) struct LayoutEditorContext {
    pub layout: Signal<Option<SpatialLayout>>,
    pub selected_zone_ids: Signal<std::collections::HashSet<String>>,
    pub hidden_zones: Signal<std::collections::HashSet<String>>,
    /// Outputs the host is transiently highlighting (e.g. the Studio rail
    /// hovering a device or channel). Rendered as a soft ring, separate
    /// from the persistent click selection.
    pub hovered_zone_ids: Signal<std::collections::HashSet<String>>,
    pub keep_aspect_ratio: Signal<bool>,
    pub set_layout: LayoutWriteHandle,
    pub set_selected_zone_ids: WriteSignal<std::collections::HashSet<String>>,
    pub set_hovered_zone_ids: WriteSignal<std::collections::HashSet<String>>,
    pub compound_depth: Signal<crate::compound_selection::CompoundDepth>,
    pub set_compound_depth: WriteSignal<crate::compound_selection::CompoundDepth>,
    pub set_is_dirty: WriteSignal<bool>,
    pub set_hidden_zones: WriteSignal<std::collections::HashSet<String>>,
    pub set_keep_aspect_ratio: WriteSignal<bool>,
    pub removed_zone_cache: Signal<crate::layout_utils::ZoneCache>,
    pub set_removed_zone_cache: WriteSignal<crate::layout_utils::ZoneCache>,
    /// Push the current in-flight layout to the daemon's preview engine.
    /// Used by the canvas during drag to keep the LED preview live without
    /// committing intermediate state to the layout signal.
    pub push_preview: Callback<SpatialLayout>,
    /// Whether the editor history has an undoable / redoable step — lets a
    /// host header drive its undo / redo buttons off this same context.
    pub can_undo: Signal<bool>,
    pub can_redo: Signal<bool>,
}

#[derive(Clone, Copy)]
pub(crate) struct LayoutZoneDisplayContext {
    pub attachment_profiles:
        LocalResource<std::collections::HashMap<String, api::DeviceComponentsResponse>>,
}

/// The layout-library controls and editor actions, lifted out of the
/// `LayoutBuilder` shell so a separately-composed header (the Studio
/// Stage) can drive the same editor. Provided via context by
/// [`LayoutEditorProvider`]; consumed by `LayoutBuilder`'s `PageHeader`
/// and by the Studio Stage's own header.
#[derive(Clone, Copy)]
pub(crate) struct LayoutEditorState {
    /// The layout currently open in the editor.
    pub layout: Signal<Option<SpatialLayout>>,
    /// Saved-layout library options for the picker, and its bound value.
    pub layout_options: Signal<Vec<(String, String)>>,
    pub layout_value: Signal<String>,
    pub set_selected_layout_id: WriteSignal<Option<String>>,
    /// Whether the open layout is the daemon's active layout.
    pub is_active: Signal<bool>,
    /// The editor write handle — undo / redo and zone mutation.
    pub write: LayoutWriteHandle,
    pub can_undo: Signal<bool>,
    pub can_redo: Signal<bool>,
    pub is_dirty: Signal<bool>,
    // Inline-edit UI state for the header controls.
    pub renaming: ReadSignal<bool>,
    pub set_renaming: WriteSignal<bool>,
    pub rename_value: ReadSignal<String>,
    pub set_rename_value: WriteSignal<String>,
    pub creating: ReadSignal<bool>,
    pub set_creating: WriteSignal<bool>,
    pub new_layout_name: ReadSignal<String>,
    pub set_new_layout_name: WriteSignal<String>,
    pub menu_open: ReadSignal<bool>,
    pub set_menu_open: WriteSignal<bool>,
    // Library actions.
    pub save: Callback<()>,
    pub revert: Callback<()>,
    pub apply: Callback<()>,
    pub delete: Callback<()>,
    pub create: Callback<()>,
    pub commit_rename: Callback<()>,
    pub duplicate: Callback<()>,
}

/// Sets up every editor signal, the history wiring, layout-library
/// persistence, and the live-preview push, then provides
/// [`LayoutEditorContext`], [`LayoutZoneDisplayContext`], and
/// [`LayoutEditorState`] to its children. Mount this once above any
/// header + [`LayoutWorkspace`] pair that should share one editor.
#[component]
pub(crate) fn LayoutEditorProvider(children: Children) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();

    // Load any UI state persisted from a previous visit so the page
    // comes back exactly the way the user left it.
    let initial_state = LayoutPageState::load();
    let remembered_layout_id = StoredValue::new(initial_state.selected_layout_id.clone());
    let per_layout_map = StoredValue::new(initial_state.per_layout);

    let (selected_layout_id, set_selected_layout_id) = signal(None::<String>);
    let (layout, set_layout_signal) = signal(None::<SpatialLayout>);
    let (saved_layout, set_saved_layout) = signal(None::<SpatialLayout>);
    let (selected_zone_ids, set_selected_zone_ids) =
        signal(std::collections::HashSet::<String>::new());
    let (compound_depth, set_compound_depth) =
        signal(crate::compound_selection::CompoundDepth::Root);
    let (creating, set_creating) = signal(false);
    let (new_layout_name, set_new_layout_name) = signal(String::new());
    let (renaming, set_renaming) = signal(false);
    let (layout_menu_open, set_layout_menu_open) = signal(false);
    let (rename_value, set_rename_value) = signal(String::new());
    let (initialized, set_initialized) = signal(false);
    let (keep_aspect_ratio, set_keep_aspect_ratio) = signal(initial_state.keep_aspect_ratio);
    let (hidden_zones, set_hidden_zones) = signal(std::collections::HashSet::<String>::new());
    let (hovered_zone_ids, set_hovered_zone_ids) =
        signal(std::collections::HashSet::<String>::new());

    let (removed_zone_cache, set_removed_zone_cache) =
        signal(crate::layout_utils::ZoneCache::new());

    // Tracked dirty flag — set true on commit, cleared on save/revert/load.
    // Replaces the old vec-equality derive that ran on every drag tick.
    let (dirty, set_is_dirty) = signal(false);
    let history = RwSignal::new(LayoutHistoryState::default());
    let set_layout = LayoutWriteHandle {
        layout,
        set_layout: set_layout_signal,
        selected_zone_ids,
        set_selected_zone_ids,
        compound_depth,
        set_compound_depth,
        removed_zone_cache,
        set_removed_zone_cache,
        history,
        set_dirty: set_is_dirty,
    };

    let layout_signal = Signal::derive(move || layout.get());
    let zone_ids_signal = Signal::derive(move || selected_zone_ids.get());
    let compound_depth_signal = Signal::derive(move || compound_depth.get());
    let keep_aspect_ratio_signal = Signal::derive(move || keep_aspect_ratio.get());
    let hidden_zones_signal = Signal::derive(move || hidden_zones.get());
    let hovered_zone_ids_signal = Signal::derive(move || hovered_zone_ids.get());
    let can_undo = Signal::derive(move || history.get().can_undo());
    let can_redo = Signal::derive(move || history.get().can_redo());

    let preview_layout = use_debounce_fn_with_arg(
        |layout: SpatialLayout| {
            leptos::task::spawn_local(async move {
                let _ = api::preview_layout(&layout).await;
            });
        },
        75.0,
    );
    let push_preview = Callback::new({
        let preview_layout = preview_layout.clone();
        move |snapshot: SpatialLayout| {
            preview_layout(snapshot);
        }
    });

    provide_context(LayoutEditorContext {
        layout: layout_signal,
        selected_zone_ids: zone_ids_signal,
        hidden_zones: hidden_zones_signal,
        hovered_zone_ids: hovered_zone_ids_signal,
        keep_aspect_ratio: keep_aspect_ratio_signal,
        set_layout,
        set_selected_zone_ids,
        set_hovered_zone_ids,
        set_is_dirty,
        set_hidden_zones,
        set_keep_aspect_ratio,
        compound_depth: compound_depth_signal,
        set_compound_depth,
        removed_zone_cache: removed_zone_cache.into(),
        set_removed_zone_cache,
        push_preview,
        can_undo,
        can_redo,
    });

    let attachment_profiles = LocalResource::new(move || {
        let current_layout = layout.get();
        let devices = ctx
            .devices_resource
            .get()
            .and_then(Result::ok)
            .unwrap_or_default();

        async move {
            let mut device_ids = std::collections::HashMap::<String, String>::new();
            if let Some(current_layout) = current_layout {
                for zone in current_layout.zones {
                    if zone.attachment.is_none() {
                        continue;
                    }
                    if let Some(device) = devices
                        .iter()
                        .find(|device| device.layout_device_id == zone.device_id)
                    {
                        device_ids.insert(zone.device_id, device.id.clone());
                    }
                }
            }

            let mut profiles = std::collections::HashMap::new();
            for (layout_device_id, device_id) in device_ids {
                if let Ok(profile) = api::fetch_device_attachments(&device_id).await {
                    profiles.insert(layout_device_id, profile);
                }
            }
            profiles
        }
    });
    provide_context(LayoutZoneDisplayContext {
        attachment_profiles,
    });

    let selected_layout_summary = Signal::derive(move || {
        let selected_id = selected_layout_id.get()?;
        let layouts = ctx.layouts_resource.get()?.ok()?;
        layouts.into_iter().find(|entry| entry.id == selected_id)
    });
    let selected_layout_is_active = Signal::derive(move || {
        selected_layout_summary
            .get()
            .is_some_and(|entry| entry.is_active)
    });
    // Tracked dirty flag — flipped explicitly on commits; saved/revert clear it.
    // Subscribers (Save/Revert buttons) only re-fire on toggle, never on drag ticks.
    let is_dirty = Signal::derive(move || dirty.get());

    // Options + current value for the layout SilkSelect. Empty value doubles
    // as "unselect current layout" — the first option is the sentinel.
    let layout_options = Signal::derive(move || {
        let Some(Ok(layouts)) = ctx.layouts_resource.get() else {
            return Vec::<(String, String)>::new();
        };
        let mut opts = vec![(String::new(), "Select layout…".to_string())];
        opts.extend(layouts.into_iter().map(|l| {
            let label = if l.is_active {
                format!("{} ({} outputs) *", l.name, l.zone_count)
            } else {
                format!("{} ({} outputs)", l.name, l.zone_count)
            };
            (l.id, label)
        }));
        opts
    });
    let layout_value = Signal::derive(move || selected_layout_id.get().unwrap_or_default());

    // Auto-select the active layout (or first available, or create a default) on mount
    Effect::new(move |_| {
        if initialized.get() {
            return;
        }
        // Wait for layouts resource to load
        let Some(Ok(layouts)) = ctx.layouts_resource.get() else {
            return;
        };
        set_initialized.set(true);

        // Prefer the layout the user was editing last, if it still exists.
        if let Some(id) = remembered_layout_id.get_value()
            && layouts.iter().any(|entry| entry.id == id)
        {
            set_selected_layout_id.set(Some(id));
            return;
        }

        if layouts.is_empty() {
            // No layouts exist — create a default one
            let layouts_resource = ctx.layouts_resource;
            let set_id = set_selected_layout_id;
            leptos::task::spawn_local(async move {
                let req = api::CreateLayoutRequest {
                    name: "Default Layout".to_string(),
                    description: None,
                    canvas_width: None,
                    canvas_height: None,
                };
                if let Ok(summary) = api::create_layout(&req).await {
                    layouts_resource.refetch();
                    set_id.set(Some(summary.id));
                }
            });
        } else {
            // Try to load the active layout first, otherwise pick the first one
            let set_id = set_selected_layout_id;
            let first_id = layouts[0].id.clone();
            leptos::task::spawn_local(async move {
                if let Ok(active) = api::fetch_active_layout().await {
                    set_id.set(Some(active.id));
                } else {
                    set_id.set(Some(first_id));
                }
            });
        }
    });

    // Load layout when selection changes. The `prev` param tracks the
    // outgoing layout id so we can flush its current zone/hidden/depth
    // state before loading the new selection — this avoids dropping
    // in-flight edits during rapid layout switches.
    Effect::new(move |prev_id: Option<Option<String>>| {
        let id = selected_layout_id.get();

        if let Some(Some(prev)) = prev_id.as_ref()
            && Some(prev) != id.as_ref()
            && initialized.get_untracked()
        {
            per_layout_map.update_value(|map| {
                map.insert(
                    prev.clone(),
                    PerLayoutState {
                        selected_zone_ids: selected_zone_ids.get_untracked(),
                        hidden_zones: hidden_zones.get_untracked(),
                        compound_depth: compound_depth.get_untracked(),
                    },
                );
            });
        }
        set_layout.reset_history();

        if let Some(fetch_id) = id.clone() {
            let set_layout = set_layout;
            let set_saved = set_saved_layout;
            leptos::task::spawn_local(async move {
                if let Ok(l) = api::fetch_layout(&fetch_id).await {
                    let layout = layout_geometry::normalize_layout_for_editor(l.clone());
                    set_saved.set(Some(layout.clone()));
                    set_layout.set(Some(layout));
                }
            });
        } else {
            set_layout.set(None);
            set_saved_layout.set(None);
        }

        // Restore (or clear) per-layout UI state for this selection.
        let loaded = id
            .as_deref()
            .and_then(|id| per_layout_map.with_value(|map| map.get(id).cloned()))
            .unwrap_or_default();
        set_selected_zone_ids.set(loaded.selected_zone_ids);
        set_hidden_zones.set(loaded.hidden_zones);
        set_compound_depth.set(loaded.compound_depth);

        id
    });

    // Persist selected layout + global UI prefs whenever they change.
    // Gated on `initialized` so we don't stomp on the remembered id
    // before the init effect has had a chance to restore it.
    Effect::new(move |_| {
        let layout_id = selected_layout_id.get();
        let keep_ar = keep_aspect_ratio.get();
        if !initialized.get_untracked() {
            return;
        }
        LayoutPageState {
            selected_layout_id: layout_id,
            keep_aspect_ratio: keep_ar,
            per_layout: per_layout_map.get_value(),
        }
        .save();
    });

    // Persist per-layout UI state (selection, hidden zones, compound
    // depth) under the current layout's id. `selected_layout_id` is
    // read untracked so this effect doesn't re-fire on layout changes —
    // that path flushes state via the `prev_id` branch above.
    Effect::new(move |_| {
        let zones = selected_zone_ids.get();
        let hidden = hidden_zones.get();
        let depth = compound_depth.get();
        if !initialized.get_untracked() {
            return;
        }
        let Some(layout_id) = selected_layout_id.get_untracked() else {
            return;
        };
        per_layout_map.update_value(|map| {
            map.insert(
                layout_id,
                PerLayoutState {
                    selected_zone_ids: zones,
                    hidden_zones: hidden,
                    compound_depth: depth,
                },
            );
        });
        LayoutPageState {
            selected_layout_id: selected_layout_id.get_untracked(),
            keep_aspect_ratio: keep_aspect_ratio.get_untracked(),
            per_layout: per_layout_map.get_value(),
        }
        .save();
    });

    // Push live preview to spatial engine whenever the layout changes (debounced).
    Effect::new(
        move |prev_snapshot: Option<Option<Vec<hypercolor_types::spatial::Output>>>| {
            let current = layout.get();
            let current_snapshot = current.as_ref().map(|current| current.zones.clone());

            // Only push preview if spatial data actually changed (avoid initial no-op).
            if current_snapshot != prev_snapshot.flatten()
                && let Some(layout) = current.as_ref()
            {
                preview_layout(layout.clone());
            }

            current_snapshot
        },
    );

    // Save handler — persists to disk via PUT + persist
    let save = Callback::new(move |()| {
        let Some(l) = layout.get_untracked() else {
            return;
        };
        let id = l.id.clone();
        let zones = l.zones.clone();
        let saved_copy = l.clone();
        let layouts_resource = ctx.layouts_resource;
        leptos::task::spawn_local(async move {
            let req = api::UpdateLayoutApiRequest {
                name: None,
                description: None,
                canvas_width: None,
                canvas_height: None,
                zones: Some(zones),
            };
            if api::update_layout(&id, &req).await.is_ok() {
                toasts::toast_success("Layout saved");
                set_saved_layout.set(Some(saved_copy));
                set_layout.mark_clean();
            } else {
                toasts::toast_error("Failed to save layout");
            }
            layouts_resource.refetch();
        });
    });

    // Revert handler — restores saved snapshot and pushes to spatial engine
    let revert = Callback::new(move |()| {
        let Some(saved) = saved_layout.get_untracked() else {
            return;
        };
        set_layout.replace_zones_with_history(saved.zones.clone());
        set_layout.mark_clean();
        toasts::toast_info("Layout reverted");
    });

    let apply = Callback::new(move |()| {
        let Some(current) = layout.get_untracked() else {
            return;
        };
        let id = current.id.clone();
        let layouts_resource = ctx.layouts_resource;
        leptos::task::spawn_local(async move {
            match api::apply_layout(&id).await {
                Ok(()) => {
                    toasts::toast_success("Layout applied");
                    layouts_resource.refetch();
                }
                Err(error) => {
                    toasts::toast_error(&format!("Apply failed: {error}"));
                }
            }
        });
    });

    // Delete handler
    let delete = Callback::new(move |()| {
        let Some(current_layout) = layout.get_untracked() else {
            return;
        };
        let layouts = ctx
            .layouts_resource
            .get_untracked()
            .and_then(Result::ok)
            .unwrap_or_default();
        let id = current_layout.id.clone();
        let name = current_layout.name.clone();
        let selected_summary = layouts.iter().find(|entry| entry.id == id).cloned();
        let fallback_layout = preferred_replacement_layout(&layouts, &id);
        let layouts_resource = ctx.layouts_resource;
        leptos::task::spawn_local(async move {
            let mut next_selection = fallback_layout.clone();

            let delete_result: Result<(), String> = async {
                if selected_summary
                    .as_ref()
                    .is_some_and(|entry| entry.is_active)
                {
                    let replacement = match fallback_layout {
                        Some(layout) => layout,
                        None => {
                            api::create_layout(&api::CreateLayoutRequest {
                                name: replacement_layout_name(&layouts),
                                description: None,
                                canvas_width: None,
                                canvas_height: None,
                            })
                            .await?
                        }
                    };
                    api::apply_layout(&replacement.id).await?;
                    next_selection = Some(replacement);
                }

                api::delete_layout(&id).await
            }
            .await;

            match delete_result {
                Ok(()) => {
                    set_selected_layout_id
                        .set(next_selection.as_ref().map(|layout| layout.id.clone()));
                    if let Some(layout) = next_selection {
                        toasts::toast_info(&format!("Deleted {name}; switched to {}", layout.name));
                    } else {
                        set_layout.set(None);
                        set_saved_layout.set(None);
                        toasts::toast_info("Layout deleted");
                    }
                    layouts_resource.refetch();
                }
                Err(e) => {
                    toasts::toast_error(&format!("Delete failed: {e}"));
                    layouts_resource.refetch();
                }
            }
        });
    });

    // Create handler
    let create = Callback::new(move |()| {
        let name = new_layout_name.get_untracked();
        if name.trim().is_empty() {
            return;
        }
        set_creating.set(false);
        set_new_layout_name.set(String::new());
        let layouts_resource = ctx.layouts_resource;
        let set_id = set_selected_layout_id;
        leptos::task::spawn_local(async move {
            let req = api::CreateLayoutRequest {
                name,
                description: None,
                canvas_width: None,
                canvas_height: None,
            };
            if let Ok(summary) = api::create_layout(&req).await {
                toasts::toast_success("Layout created");
                layouts_resource.refetch();
                set_id.set(Some(summary.id));
            }
        });
    });

    // Rename handler — persists name change immediately via API
    let commit_rename = Callback::new(move |()| {
        let name = rename_value.get_untracked();
        let name = name.trim().to_string();
        if name.is_empty() {
            set_renaming.set(false);
            return;
        }
        let Some(current) = layout.get_untracked() else {
            set_renaming.set(false);
            return;
        };
        // Skip if name hasn't changed
        if name == current.name {
            set_renaming.set(false);
            return;
        }
        let id = current.id.clone();
        let new_name = name.clone();
        let layouts_resource = ctx.layouts_resource;
        set_renaming.set(false);
        leptos::task::spawn_local(async move {
            let req = api::UpdateLayoutApiRequest {
                name: Some(new_name.clone()),
                description: None,
                canvas_width: None,
                canvas_height: None,
                zones: None,
            };
            match api::update_layout(&id, &req).await {
                Ok(_) => {
                    // Update the in-memory layout name so the dropdown reflects it immediately
                    set_layout.update_without_history(|l| {
                        if let Some(layout) = l {
                            layout.name.clone_from(&new_name);
                        }
                    });
                    set_saved_layout.update(|l| {
                        if let Some(layout) = l {
                            layout.name.clone_from(&new_name);
                        }
                    });
                    toasts::toast_success("Layout renamed");
                    layouts_resource.refetch();
                }
                Err(e) => {
                    toasts::toast_error(&format!("Rename failed: {e}"));
                }
            }
        });
    });

    // Duplicate handler — creates a copy of the current layout with zones
    let duplicate = Callback::new(move |()| {
        let Some(current) = layout.get_untracked() else {
            return;
        };
        let new_name = format!("{} (copy)", current.name);
        let zones = current.zones.clone();
        let canvas_width = current.canvas_width;
        let canvas_height = current.canvas_height;
        let layouts_resource = ctx.layouts_resource;
        let set_id = set_selected_layout_id;
        leptos::task::spawn_local(async move {
            let req = api::CreateLayoutRequest {
                name: new_name,
                description: None,
                canvas_width: Some(canvas_width),
                canvas_height: Some(canvas_height),
            };
            match api::create_layout(&req).await {
                Ok(summary) => {
                    // Update the new layout with zones from the original
                    let update_req = api::UpdateLayoutApiRequest {
                        name: None,
                        description: None,
                        canvas_width: None,
                        canvas_height: None,
                        zones: Some(zones),
                    };
                    let _ = api::update_layout(&summary.id, &update_req).await;
                    toasts::toast_success("Layout duplicated");
                    layouts_resource.refetch();
                    set_id.set(Some(summary.id));
                }
                Err(e) => {
                    toasts::toast_error(&format!("Duplicate failed: {e}"));
                }
            }
        });
    });

    provide_context(LayoutEditorState {
        layout: layout_signal,
        layout_options,
        layout_value,
        set_selected_layout_id,
        is_active: selected_layout_is_active,
        write: set_layout,
        can_undo,
        can_redo,
        is_dirty,
        renaming,
        set_renaming,
        rename_value,
        set_rename_value,
        creating,
        set_creating,
        new_layout_name,
        set_new_layout_name,
        menu_open: layout_menu_open,
        set_menu_open: set_layout_menu_open,
        save,
        revert,
        apply,
        delete,
        create,
        commit_rename,
        duplicate,
    });

    children()
}

/// The `/layout` page header — the saved-layout picker, rename / new
/// controls, undo / redo, Revert / Save, and the per-layout action
/// kebab, wrapped in a `PageHeader`. Reads everything from the
/// context-provided [`LayoutEditorState`]; the Studio Stage composes a
/// different header around the same state.
#[component]
fn LayoutBuilderHeader() -> impl IntoView {
    let state = expect_context::<LayoutEditorState>();
    let layout = state.layout;
    let is_dirty = state.is_dirty;
    let can_undo = state.can_undo;
    let can_redo = state.can_redo;
    let renaming = state.renaming;
    let creating = state.creating;
    let layout_menu_open = state.menu_open;
    let selected_layout_is_active = state.is_active;

    view! {
        <PageHeader
            icon=LuLayoutTemplate
            title="Layout"
            tagline="Arrange devices on the canvas"
            accent=PageAccent::Coral
        >
            <HeaderTrailing slot>
                // Single-line action cluster: [Undo][Redo]  [Revert][Save].
                // Save doubles as the dirty indicator — glows when there
                // are unsaved changes, dims when clean. Revert follows the
                // same active/disabled pattern. No separate dirty strip.
                {move || layout.get().map(|_| {
                    let dirty = is_dirty.get();
                    let save_style = if dirty {
                        "background: rgba(80, 250, 123, 0.14); border-color: rgba(80, 250, 123, 0.35); color: rgb(80, 250, 123); \
                         box-shadow: 0 0 14px rgba(80, 250, 123, 0.18)"
                    } else {
                        "background: var(--color-surface-overlay); border-color: var(--color-border-subtle); color: var(--color-text-tertiary); opacity: 0.4; pointer-events: none"
                    };
                    let revert_style = if dirty {
                        "background: rgba(241, 250, 140, 0.08); border-color: rgba(241, 250, 140, 0.25); color: rgb(241, 250, 140)"
                    } else {
                        "background: var(--color-surface-overlay); border-color: var(--color-border-subtle); color: var(--color-text-tertiary); opacity: 0.4; pointer-events: none"
                    };
                    view! {
                        <div class="flex items-center gap-2">
                            <div class="flex items-center gap-1">
                                <button
                                    class="w-8 h-8 flex items-center justify-center rounded-md text-fg-tertiary
                                           hover:text-fg-primary hover:bg-surface-hover/40 transition-all btn-press
                                           disabled:opacity-30 disabled:pointer-events-none"
                                    title="Undo (Ctrl+Z)"
                                    on:click=move |_| state.write.undo()
                                    disabled=move || !can_undo.get()
                                >
                                    <Icon icon=LuUndo2 width="15px" height="15px" />
                                </button>
                                <button
                                    class="w-8 h-8 flex items-center justify-center rounded-md text-fg-tertiary
                                           hover:text-fg-primary hover:bg-surface-hover/40 transition-all btn-press
                                           disabled:opacity-30 disabled:pointer-events-none"
                                    title="Redo (Ctrl+Shift+Z)"
                                    on:click=move |_| state.write.redo()
                                    disabled=move || !can_redo.get()
                                >
                                    <Icon icon=LuRedo2 width="15px" height="15px" />
                                </button>
                            </div>
                            <div class="w-px h-5 bg-edge-subtle/40 mx-1" />
                            <button
                                class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium border transition-all btn-press"
                                style=revert_style
                                on:click=move |_| state.revert.run(())
                                disabled=move || !is_dirty.get()
                            >
                                <Icon icon=LuUndo2 width="14px" height="14px" />
                                "Revert"
                            </button>
                            <button
                                class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium border transition-all btn-press"
                                style=save_style
                                on:click=move |_| state.save.run(())
                                disabled=move || !is_dirty.get()
                            >
                                <Icon icon=LuSave width="14px" height="14px" />
                                "Save"
                            </button>
                        </div>
                    }
                })}
            </HeaderTrailing>
            <HeaderToolbar slot>
                <div class="flex items-center gap-3">

                {move || if renaming.get() {
                    // Inline rename input
                    view! {
                        <div class="flex items-center gap-2 animate-enter-down">
                            <input
                                type="text"
                                class="bg-surface-sunken border border-edge-subtle rounded-lg px-3 py-1.5 text-sm text-fg-primary
                                       placeholder-fg-tertiary focus:outline-none focus:border-accent-muted glow-ring w-52 transition-all"
                                prop:value=move || state.rename_value.get()
                                autofocus=true
                                on:input=move |ev| {
                                    let event = Input::from_event(ev);
                                    if let Some(value) = event.value_string() {
                                        state.set_rename_value.set(value);
                                    }
                                }
                                on:blur=move |_| state.commit_rename.run(())
                                on:keydown=move |ev: web_sys::KeyboardEvent| {
                                    if ev.key() == "Enter" {
                                        state.commit_rename.run(());
                                    } else if ev.key() == "Escape" {
                                        state.set_renaming.set(false);
                                    }
                                }
                            />
                        </div>
                    }.into_any()
                } else {
                    // Normal dropdown selector + rename button
                    view! {
                        <div class="flex items-center gap-2">
                            <div class="min-w-[200px]">
                                <SilkSelect
                                    value=state.layout_value
                                    options=state.layout_options
                                    on_change=Callback::new(move |val: String| {
                                        if val.is_empty() {
                                            state.set_selected_layout_id.set(None);
                                        } else {
                                            state.set_selected_layout_id.set(Some(val));
                                        }
                                    })
                                    placeholder="Loading layouts…"
                                    class="bg-surface-sunken border border-edge-subtle px-3 py-1.5 text-sm text-fg-primary glow-ring"
                                />
                            </div>

                            // Rename button — only when a layout is selected
                            <Show when=move || layout.with(|l| l.is_some())>
                                <button
                                    class="p-1.5 rounded-md text-fg-tertiary hover:text-fg-primary hover:bg-surface-hover/40
                                           transition-all btn-press"
                                    title="Rename layout"
                                    on:click=move |_| {
                                        if let Some(current) = layout.get_untracked() {
                                            state.set_rename_value.set(current.name.clone());
                                            state.set_renaming.set(true);
                                        }
                                    }
                                >
                                    <Icon icon=LuPencil width="14px" height="14px" />
                                </button>
                            </Show>
                        </div>
                    }.into_any()
                }}

            </div>

            // New layout button / inline form
            {move || if creating.get() {
                view! {
                    <div class="flex items-center gap-2 animate-enter-down">
                        <input
                            type="text"
                            placeholder="Layout name"
                            class="bg-surface-sunken border border-edge-subtle rounded-lg px-3 py-1.5 text-sm text-fg-primary
                                   placeholder-fg-tertiary focus:outline-none focus:border-accent-muted glow-ring w-40 transition-all"
                            prop:value=move || state.new_layout_name.get()
                            on:input=move |ev| {
                                let event = Input::from_event(ev);
                                if let Some(value) = event.value_string() {
                                    state.set_new_layout_name.set(value);
                                }
                            }
                            on:keydown=move |ev| {
                                if ev.key() == "Enter" { state.create.run(()); }
                                if ev.key() == "Escape" { state.set_creating.set(false); }
                            }
                        />
                        <button
                            class="px-3 py-1.5 rounded-lg text-xs font-medium border transition-all btn-press"
                            style="background: rgba(80, 250, 123, 0.1); border-color: rgba(80, 250, 123, 0.2); color: rgb(80, 250, 123)"
                            on:click=move |_| state.create.run(())
                        >"Create"</button>
                        <button
                            class="px-3 py-1.5 rounded-lg text-xs font-medium bg-surface-overlay/40 border border-edge-subtle
                                   text-fg-tertiary hover:text-fg-primary hover:bg-surface-hover/40 transition-all btn-press"
                            on:click=move |_| state.set_creating.set(false)
                        >"Cancel"</button>
                    </div>
                }.into_any()
            } else {
                view! {
                    <button
                        class="flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-medium border whitespace-nowrap transition-all btn-press"
                        style="background: rgba(225, 53, 255, 0.08); border-color: rgba(225, 53, 255, 0.2); color: rgb(225, 53, 255)"
                        on:click=move |_| state.set_creating.set(true)
                    >
                        <Icon icon=LuPlus width="12px" height="12px" />
                        "New"
                    </button>
                }.into_any()
            }}

            <div class="flex-1" />

            // Overflow menu — per-layout actions (Apply, Duplicate, Delete)
            // collapsed into a single kebab. Keeps the toolbar row quiet
            // during normal use; the popover opens on demand.
            {move || layout.get().map(|_| view! {
                <div class="relative layout-action-menu">
                    <button
                        class="w-8 h-8 flex items-center justify-center rounded-md
                               text-fg-tertiary hover:text-fg-primary hover:bg-surface-hover/40
                               transition-all btn-press"
                        title="Layout actions"
                        on:click=move |_| state.set_menu_open.update(|v| *v = !*v)
                    >
                        <Icon icon=LuEllipsis width="15px" height="15px" />
                    </button>
                    <Show when=move || layout_menu_open.get()>
                        <ControlDropdownDismissHandlers
                            class_name="layout-action-menu".to_string()
                            is_open=layout_menu_open
                            set_open=state.set_menu_open
                        />
                        <div
                            class="absolute right-0 top-full mt-1 z-[100] w-48
                                   rounded-lg overflow-hidden
                                   bg-surface-overlay/98 backdrop-blur-xl
                                   border border-edge-subtle dropdown-glow
                                   animate-enter-down"
                            on:keydown=move |ev: web_sys::KeyboardEvent| {
                                if ev.key() == "Escape" {
                                    state.set_menu_open.set(false);
                                }
                            }
                        >
                            // Apply / Active — reflects the live state of this layout.
                            // When active, shows as a green read-only marker.
                            // When inactive + clean, shows "Apply" as an actionable button.
                            // When inactive + dirty, hides (save first).
                            <Show when=move || selected_layout_is_active.get()>
                                <div class="w-full px-3 py-2 text-xs flex items-center gap-2
                                            text-fg-tertiary cursor-default">
                                    <Icon icon=LuCheck width="12px" height="12px"
                                          style="color: rgb(80, 250, 123); flex-shrink: 0" />
                                    <span>"Active"</span>
                                </div>
                            </Show>
                            <Show when=move || !selected_layout_is_active.get() && !is_dirty.get()>
                                <button
                                    class="dropdown-option w-full text-left px-3 py-2 text-xs cursor-pointer
                                           flex items-center gap-2 text-fg-secondary hover:text-fg-primary"
                                    on:click=move |_| {
                                        state.apply.run(());
                                        state.set_menu_open.set(false);
                                    }
                                >
                                    <Icon icon=LuCheck width="12px" height="12px"
                                          style="color: rgb(128, 255, 234); flex-shrink: 0" />
                                    <span>"Apply"</span>
                                </button>
                            </Show>
                            <button
                                class="dropdown-option w-full text-left px-3 py-2 text-xs cursor-pointer
                                       flex items-center gap-2 text-fg-secondary hover:text-fg-primary"
                                on:click=move |_| {
                                    if let Some(current) = layout.get_untracked() {
                                        state.set_rename_value.set(current.name.clone());
                                        state.set_renaming.set(true);
                                    }
                                    state.set_menu_open.set(false);
                                }
                            >
                                <Icon icon=LuPencil width="12px" height="12px"
                                      style="color: rgba(139, 133, 160, 0.7); flex-shrink: 0" />
                                <span>"Rename"</span>
                            </button>
                            <button
                                class="dropdown-option w-full text-left px-3 py-2 text-xs cursor-pointer
                                       flex items-center gap-2 text-fg-secondary hover:text-fg-primary"
                                on:click=move |_| {
                                    state.duplicate.run(());
                                    state.set_menu_open.set(false);
                                }
                            >
                                <Icon icon=LuCopy width="12px" height="12px"
                                      style="color: rgba(128, 255, 234, 0.7); flex-shrink: 0" />
                                <span>"Duplicate"</span>
                            </button>
                            <div class="h-px bg-edge-subtle/40 mx-2 my-1" />
                            <button
                                class="dropdown-option w-full text-left px-3 py-2 text-xs cursor-pointer
                                       flex items-center gap-2 text-status-error/70 hover:text-status-error"
                                on:click=move |_| {
                                    state.delete.run(());
                                    state.set_menu_open.set(false);
                                }
                            >
                                <Icon icon=LuTrash2 width="12px" height="12px"
                                      style="color: rgba(255, 99, 99, 0.7); flex-shrink: 0" />
                                <span>"Delete"</span>
                            </button>
                        </div>
                    </Show>
                </div>
            })}
            </HeaderToolbar>
        </PageHeader>
    }
}

/// The headless layout editor body — the device palette, the canvas
/// viewport, and the zone-properties panel, with their resizable-panel
/// state. Carries no header; mount it under a [`LayoutEditorProvider`]
/// beside whatever header the host wants.
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

/// Layout builder — the `/layout` page: editor state, its `PageHeader`,
/// and the headless `LayoutWorkspace`, all under one provider.
#[component]
pub fn LayoutBuilder(
    /// Compact embedding (Studio Stage). Forwarded to [`LayoutWorkspace`].
    #[prop(optional)]
    compact: bool,
) -> impl IntoView {
    view! {
        <LayoutEditorProvider>
            <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
                <LayoutBuilderHeader />
                <LayoutWorkspace compact=compact />
            </div>
        </LayoutEditorProvider>
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

/// Sets up the editor signals, history, and live-preview wiring for the
/// Studio Stage, scoped to the **selected zone's** own `SpatialLayout`.
///
/// Where [`LayoutEditorProvider`] edits the standalone layouts library,
/// this provider loads the selected zone's `Zone.layout` and
/// persists it through the per-zone layout API (`PUT
/// .../zones/{id}/layout` — a placement merge, plan 55 §5.1). Switching
/// zones switches the canvas. Mount it once above the Stage; it provides
/// [`LayoutEditorContext`], [`LayoutZoneDisplayContext`], and
/// [`ZoneCanvasActions`].
#[component]
pub(crate) fn ZoneLayoutProvider(
    /// The active scene — the source of the zone set and the
    /// `groups_revision` carried as each save's `If-Match` precondition.
    #[prop(into)]
    active_scene: Signal<Option<api::ActiveSceneResponse>>,
    /// The selected zone's id (a `Zone` id). `None`, an unknown
    /// id, or a Display zone leaves the canvas empty.
    #[prop(into)]
    selected_zone_id: Signal<Option<String>>,
    /// Re-fetch the active scene after a save so the tree and Stage pick
    /// up the new `groups_revision`.
    refresh_scene: Callback<()>,
    children: Children,
) -> impl IntoView {
    let devices_ctx = expect_context::<DevicesContext>();
    let ws_ctx = expect_context::<WsContext>();

    let (layout, set_layout_signal) = signal(None::<SpatialLayout>);
    let (saved_layout, set_saved_layout) = signal(None::<SpatialLayout>);
    let (selected_zone_ids, set_selected_zone_ids) =
        signal(std::collections::HashSet::<String>::new());
    let (compound_depth, set_compound_depth) =
        signal(crate::compound_selection::CompoundDepth::Root);
    let (keep_aspect_ratio, set_keep_aspect_ratio) = signal(false);
    let (hidden_zones, set_hidden_zones) = signal(std::collections::HashSet::<String>::new());
    let (hovered_zone_ids, set_hovered_zone_ids) =
        signal(std::collections::HashSet::<String>::new());
    let (removed_zone_cache, set_removed_zone_cache) =
        signal(crate::layout_utils::ZoneCache::new());
    let (dirty, set_is_dirty) = signal(false);
    let history = RwSignal::new(LayoutHistoryState::default());

    let set_layout = LayoutWriteHandle {
        layout,
        set_layout: set_layout_signal,
        selected_zone_ids,
        set_selected_zone_ids,
        compound_depth,
        set_compound_depth,
        removed_zone_cache,
        set_removed_zone_cache,
        history,
        set_dirty: set_is_dirty,
    };

    let layout_signal = Signal::derive(move || layout.get());
    let can_undo = Signal::derive(move || history.get().can_undo());
    let can_redo = Signal::derive(move || history.get().can_redo());
    let is_dirty = Signal::derive(move || dirty.get());

    let active_preview_key = StoredValue::new(None::<(String, String)>);
    let push_preview = Callback::new(move |snapshot: SpatialLayout| {
        let Some(scene_id) =
            active_scene.with_untracked(|scene| scene.as_ref().map(|scene| scene.id.clone()))
        else {
            return;
        };
        let Some(zone_id) = selected_zone_id.get_untracked() else {
            return;
        };
        active_preview_key.set_value(Some((scene_id.clone(), zone_id.clone())));
        ws_ctx
            .send_zone_layout_preview
            .run((scene_id, zone_id, snapshot));
    });

    Effect::new(move |_| {
        let next_key = active_scene
            .with(|scene| scene.as_ref().map(|scene| scene.id.clone()))
            .zip(selected_zone_id.get());
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

    provide_context(LayoutEditorContext {
        layout: layout_signal,
        selected_zone_ids: Signal::derive(move || selected_zone_ids.get()),
        hidden_zones: Signal::derive(move || hidden_zones.get()),
        hovered_zone_ids: Signal::derive(move || hovered_zone_ids.get()),
        keep_aspect_ratio: Signal::derive(move || keep_aspect_ratio.get()),
        set_layout,
        set_selected_zone_ids,
        set_hovered_zone_ids,
        set_is_dirty,
        set_hidden_zones,
        set_keep_aspect_ratio,
        compound_depth: Signal::derive(move || compound_depth.get()),
        set_compound_depth,
        removed_zone_cache: removed_zone_cache.into(),
        set_removed_zone_cache,
        push_preview,
        can_undo,
        can_redo,
    });

    let attachment_profiles = LocalResource::new(move || {
        let current_layout = layout.get();
        let devices = devices_ctx
            .devices_resource
            .get()
            .and_then(Result::ok)
            .unwrap_or_default();

        async move {
            let mut device_ids = std::collections::HashMap::<String, String>::new();
            if let Some(current_layout) = current_layout {
                for zone in current_layout.zones {
                    if zone.attachment.is_none() {
                        continue;
                    }
                    if let Some(device) = devices
                        .iter()
                        .find(|device| device.layout_device_id == zone.device_id)
                    {
                        device_ids.insert(zone.device_id, device.id.clone());
                    }
                }
            }

            let mut profiles = std::collections::HashMap::new();
            for (layout_device_id, device_id) in device_ids {
                if let Ok(profile) = api::fetch_device_attachments(&device_id).await {
                    profiles.insert(layout_device_id, profile);
                }
            }
            profiles
        }
    });
    provide_context(LayoutZoneDisplayContext {
        attachment_profiles,
    });

    // Reload the canvas when the zone changes, or when the selected
    // zone's OUTPUT SET changes (a device assigned / removed elsewhere).
    // A placement-only change — including this canvas's own saved edits —
    // leaves the signature unchanged, so an unrelated scene refetch never
    // clobbers in-flight canvas edits.
    let zone_signature = Memo::new(move |_| {
        let zone_id = selected_zone_id.get()?;
        active_scene.with(|scene| {
            let group = scene
                .as_ref()?
                .groups
                .iter()
                .find(|group| group.id.to_string() == zone_id)?;
            if group.role == ZoneRole::Display {
                return None;
            }
            let mut output_ids: Vec<String> = group
                .layout
                .zones
                .iter()
                .map(|output| output.id.clone())
                .collect();
            output_ids.sort();
            Some((zone_id, output_ids))
        })
    });

    Effect::new(move |_| {
        set_layout.reset_history();
        set_selected_zone_ids.set(std::collections::HashSet::new());
        set_hidden_zones.set(std::collections::HashSet::new());
        set_compound_depth.set(crate::compound_selection::CompoundDepth::Root);

        let Some((zone_id, _)) = zone_signature.get() else {
            set_layout.set(None);
            set_saved_layout.set(None);
            return;
        };
        let loaded = active_scene.with_untracked(|scene| {
            scene.as_ref().and_then(|scene| {
                scene
                    .groups
                    .iter()
                    .find(|group| group.id.to_string() == zone_id)
                    .map(|group| group.layout.clone())
            })
        });
        match loaded {
            Some(layout) => {
                let layout = layout_geometry::normalize_layout_for_editor(layout);
                set_saved_layout.set(Some(layout.clone()));
                set_layout.set(Some(layout));
            }
            None => {
                set_layout.set(None);
                set_saved_layout.set(None);
            }
        }
    });

    let save = Callback::new(move |()| {
        let Some(current) = layout.get_untracked() else {
            return;
        };
        let Some(zone_id) = selected_zone_id.get_untracked() else {
            return;
        };
        let Some((scene_id, revision)) = active_scene
            .get_untracked()
            .map(|scene| (scene.id, scene.groups_revision))
        else {
            return;
        };
        leptos::task::spawn_local(async move {
            match api::zones::update_zone_layout(&scene_id, &zone_id, &current, Some(revision))
                .await
            {
                Ok(api::zones::ZoneOutcome::Applied(_)) => {
                    set_saved_layout.set(Some(current));
                    set_layout.mark_clean();
                    ws_ctx
                        .clear_zone_layout_preview
                        .run((scene_id.clone(), zone_id.clone()));
                    active_preview_key.set_value(None);
                    toasts::toast_success("Zone layout saved");
                    refresh_scene.run(());
                }
                Ok(api::zones::ZoneOutcome::Stale { .. }) => {
                    ws_ctx
                        .clear_zone_layout_preview
                        .run((scene_id.clone(), zone_id.clone()));
                    active_preview_key.set_value(None);
                    toasts::toast_error("Scene changed elsewhere — reloaded, try again");
                    refresh_scene.run(());
                }
                Err(error) => toasts::toast_error(&format!("Save failed: {error}")),
            }
        });
    });

    let revert = Callback::new(move |()| {
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
