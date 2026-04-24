//! Layout builder wrapper — toolbar + three-column layout editor.
//!
//! Edits are pushed to the spatial engine immediately for live preview.
//! Save persists to disk. Revert restores to the last saved state.

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::use_debounce_fn_with_arg;

use hypercolor_leptos_ext::events::{Input, target_is_text_entry};
use crate::api;
use crate::app::DevicesContext;
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
use crate::toasts;
use hypercolor_types::spatial::{DeviceZone, SpatialLayout};

// Panel size defaults and constraints
const SIDEBAR_DEFAULT: f64 = 280.0;
const SIDEBAR_MIN: f64 = 180.0;
const SIDEBAR_MAX: f64 = 480.0;
const BOTTOM_DEFAULT: f64 = 160.0;
const BOTTOM_MIN: f64 = 96.0;
const BOTTOM_MAX: f64 = 500.0;

const LS_KEY_SIDEBAR: &str = "hc-layout-sidebar-width";
const LS_KEY_BOTTOM: &str = "hc-layout-bottom-height";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

fn load_panel_size(key: &str, default: f64, min: f64, max: f64) -> f64 {
    storage()
        .and_then(|s| s.get_item(key).ok().flatten())
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v.clamp(min, max))
        .unwrap_or(default)
}

fn save_panel_size(key: &str, value: f64) {
    if let Some(s) = storage() {
        let _ = s.set_item(key, &format!("{value:.0}"));
    }
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

    pub fn update(self, f: impl FnOnce(&mut Option<SpatialLayout>)) {
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

    pub fn replace_zones_with_history(self, zones: Vec<DeviceZone>) {
        self.update(move |current| {
            if let Some(layout) = current {
                layout.zones = zones;
            }
        });
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
        }
    }
}

/// Shared layout editor state — provided via context to palette, canvas, and zone properties.
#[derive(Clone, Copy)]
pub(crate) struct LayoutEditorContext {
    pub layout: Signal<Option<SpatialLayout>>,
    pub selected_zone_ids: Signal<std::collections::HashSet<String>>,
    pub hidden_zones: Signal<std::collections::HashSet<String>>,
    pub keep_aspect_ratio: Signal<bool>,
    pub set_layout: LayoutWriteHandle,
    pub set_selected_zone_ids: WriteSignal<std::collections::HashSet<String>>,
    pub compound_depth: Signal<crate::compound_selection::CompoundDepth>,
    pub set_compound_depth: WriteSignal<crate::compound_selection::CompoundDepth>,
    pub set_is_dirty: WriteSignal<bool>,
    pub set_hidden_zones: WriteSignal<std::collections::HashSet<String>>,
    pub set_keep_aspect_ratio: WriteSignal<bool>,
    pub removed_zone_cache: Signal<crate::layout_utils::ZoneCache>,
    pub set_removed_zone_cache: WriteSignal<crate::layout_utils::ZoneCache>,
}

#[derive(Clone, Copy)]
pub(crate) struct LayoutZoneDisplayContext {
    pub attachment_profiles:
        LocalResource<std::collections::HashMap<String, api::DeviceAttachmentsResponse>>,
}

/// Layout builder — wraps toolbar, palette, canvas viewport, and zone properties.
#[component]
pub fn LayoutBuilder() -> impl IntoView {
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

    let (removed_zone_cache, set_removed_zone_cache) =
        signal(crate::layout_utils::ZoneCache::new());

    // Writable dirty signal for child components (actual dirty state is derived
    // from layout vs saved_layout comparison, but children need to signal changes).
    let (_dirty_marker, set_is_dirty) = signal(false);
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
    };

    let layout_signal = Signal::derive(move || layout.get());
    let zone_ids_signal = Signal::derive(move || selected_zone_ids.get());
    let compound_depth_signal = Signal::derive(move || compound_depth.get());
    let keep_aspect_ratio_signal = Signal::derive(move || keep_aspect_ratio.get());
    let hidden_zones_signal = Signal::derive(move || hidden_zones.get());
    let has_layout = Signal::derive(move || layout.with(|current| current.is_some()));

    provide_context(LayoutEditorContext {
        layout: layout_signal,
        selected_zone_ids: zone_ids_signal,
        hidden_zones: hidden_zones_signal,
        keep_aspect_ratio: keep_aspect_ratio_signal,
        set_layout,
        set_selected_zone_ids,
        set_is_dirty,
        set_hidden_zones,
        set_keep_aspect_ratio,
        compound_depth: compound_depth_signal,
        set_compound_depth,
        removed_zone_cache: removed_zone_cache.into(),
        set_removed_zone_cache,
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
    let can_undo = Signal::derive(move || history.get().can_undo());
    let can_redo = Signal::derive(move || history.get().can_redo());
    // Derive dirty state by comparing working layout to saved snapshot.
    let is_dirty = Signal::derive(move || {
        let current = layout.get();
        let saved = saved_layout.get();
        match (current, saved) {
            (Some(c), Some(s)) => c.zones != s.zones,
            _ => false,
        }
    });

    // Options + current value for the layout SilkSelect. Empty value doubles
    // as "unselect current layout" — the first option is the sentinel.
    let layout_options = Signal::derive(move || {
        let Some(Ok(layouts)) = ctx.layouts_resource.get() else {
            return Vec::<(String, String)>::new();
        };
        let mut opts = vec![(String::new(), "Select layout…".to_string())];
        opts.extend(layouts.into_iter().map(|l| {
            let label = if l.is_active {
                format!("{} ({} zones) *", l.name, l.zone_count)
            } else {
                format!("{} ({} zones)", l.name, l.zone_count)
            };
            (l.id, label)
        }));
        opts
    });
    let layout_value = Signal::derive(move || selected_layout_id.get().unwrap_or_default());

    let preview_layout = use_debounce_fn_with_arg(
        |layout: SpatialLayout| {
            leptos::task::spawn_local(async move {
                let _ = api::preview_layout(&layout).await;
            });
        },
        75.0,
    );
    let _history_shortcuts =
        window_event_listener(ev::keydown, move |ev: web_sys::KeyboardEvent| {
            if keyboard_target_is_text_input(ev.target()) {
                return;
            }
            if ev.alt_key() || !(ev.ctrl_key() || ev.meta_key()) {
                return;
            }
            match ev.key().as_str() {
                "z" | "Z" if ev.shift_key() => {
                    if can_redo.get_untracked() {
                        ev.prevent_default();
                        set_layout.redo();
                    }
                }
                "z" | "Z" => {
                    if can_undo.get_untracked() {
                        ev.prevent_default();
                        set_layout.undo();
                    }
                }
                "y" | "Y" => {
                    if can_redo.get_untracked() {
                        ev.prevent_default();
                        set_layout.redo();
                    }
                }
                _ => {}
            }
        });

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
        move |prev_snapshot: Option<Option<Vec<hypercolor_types::spatial::DeviceZone>>>| {
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
    let save_layout = move || {
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
            } else {
                toasts::toast_error("Failed to save layout");
            }
            layouts_resource.refetch();
        });
    };

    // Revert handler — restores saved snapshot and pushes to spatial engine
    let revert_layout = move || {
        let Some(saved) = saved_layout.get_untracked() else {
            return;
        };
        set_layout.replace_zones_with_history(saved.zones.clone());
        toasts::toast_info("Layout reverted");
    };

    let apply_layout = move || {
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
    };

    // Delete handler
    let delete_layout = move || {
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
    };

    // Create handler
    let create_layout = move || {
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
    };

    // Rename handler — persists name change immediately via API
    let commit_rename = move || {
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
    };

    // Duplicate handler — creates a copy of the current layout with zones
    let duplicate_layout = move || {
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
    };

    view! {
        <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
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
                                        on:click=move |_| set_layout.undo()
                                        disabled=move || !can_undo.get()
                                    >
                                        <Icon icon=LuUndo2 width="15px" height="15px" />
                                    </button>
                                    <button
                                        class="w-8 h-8 flex items-center justify-center rounded-md text-fg-tertiary
                                               hover:text-fg-primary hover:bg-surface-hover/40 transition-all btn-press
                                               disabled:opacity-30 disabled:pointer-events-none"
                                        title="Redo (Ctrl+Shift+Z)"
                                        on:click=move |_| set_layout.redo()
                                        disabled=move || !can_redo.get()
                                    >
                                        <Icon icon=LuRedo2 width="15px" height="15px" />
                                    </button>
                                </div>
                                <div class="w-px h-5 bg-edge-subtle/40 mx-1" />
                                <button
                                    class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium border transition-all btn-press"
                                    style=revert_style
                                    on:click=move |_| revert_layout()
                                    disabled=move || !is_dirty.get()
                                >
                                    <Icon icon=LuUndo2 width="14px" height="14px" />
                                    "Revert"
                                </button>
                                <button
                                    class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium border transition-all btn-press"
                                    style=save_style
                                    on:click=move |_| save_layout()
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
                            <div class="flex items-center gap-2 animate-slide-down">
                                <input
                                    type="text"
                                    class="bg-surface-sunken border border-edge-subtle rounded-lg px-3 py-1.5 text-sm text-fg-primary
                                           placeholder-fg-tertiary focus:outline-none focus:border-accent-muted glow-ring w-52 transition-all"
                                    prop:value=move || rename_value.get()
                                    autofocus=true
                                    on:input=move |ev| {
                                        let event = Input::from_event(ev);
                                        if let Some(value) = event.value_string() {
                                            set_rename_value.set(value);
                                        }
                                    }
                                    on:blur=move |_| commit_rename()
                                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                                        if ev.key() == "Enter" {
                                            commit_rename();
                                        } else if ev.key() == "Escape" {
                                            set_renaming.set(false);
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
                                        value=layout_value
                                        options=layout_options
                                        on_change=Callback::new(move |val: String| {
                                            if val.is_empty() {
                                                set_selected_layout_id.set(None);
                                            } else {
                                                set_selected_layout_id.set(Some(val));
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
                                                set_rename_value.set(current.name.clone());
                                                set_renaming.set(true);
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
                        <div class="flex items-center gap-2 animate-slide-down">
                            <input
                                type="text"
                                placeholder="Layout name"
                                class="bg-surface-sunken border border-edge-subtle rounded-lg px-3 py-1.5 text-sm text-fg-primary
                                       placeholder-fg-tertiary focus:outline-none focus:border-accent-muted glow-ring w-40 transition-all"
                                prop:value=move || new_layout_name.get()
                                on:input=move |ev| {
                                    let event = Input::from_event(ev);
                                    if let Some(value) = event.value_string() {
                                        set_new_layout_name.set(value);
                                    }
                                }
                                on:keydown=move |ev| {
                                    if ev.key() == "Enter" { create_layout(); }
                                    if ev.key() == "Escape" { set_creating.set(false); }
                                }
                            />
                            <button
                                class="px-3 py-1.5 rounded-lg text-xs font-medium border transition-all btn-press"
                                style="background: rgba(80, 250, 123, 0.1); border-color: rgba(80, 250, 123, 0.2); color: rgb(80, 250, 123)"
                                on:click=move |_| create_layout()
                            >"Create"</button>
                            <button
                                class="px-3 py-1.5 rounded-lg text-xs font-medium bg-surface-overlay/40 border border-edge-subtle
                                       text-fg-tertiary hover:text-fg-primary hover:bg-surface-hover/40 transition-all btn-press"
                                on:click=move |_| set_creating.set(false)
                            >"Cancel"</button>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <button
                            class="flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-medium border whitespace-nowrap transition-all btn-press"
                            style="background: rgba(225, 53, 255, 0.08); border-color: rgba(225, 53, 255, 0.2); color: rgb(225, 53, 255)"
                            on:click=move |_| set_creating.set(true)
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
                            on:click=move |_| set_layout_menu_open.update(|v| *v = !*v)
                        >
                            <Icon icon=LuEllipsis width="15px" height="15px" />
                        </button>
                        <Show when=move || layout_menu_open.get()>
                            <ControlDropdownDismissHandlers
                                class_name="layout-action-menu".to_string()
                                is_open=layout_menu_open
                                set_open=set_layout_menu_open
                            />
                            <div
                                class="absolute right-0 top-full mt-1 z-[100] w-48
                                       rounded-lg overflow-hidden
                                       bg-surface-overlay/98 backdrop-blur-xl
                                       border border-edge-subtle dropdown-glow
                                       animate-slide-down"
                                on:keydown=move |ev: web_sys::KeyboardEvent| {
                                    if ev.key() == "Escape" {
                                        set_layout_menu_open.set(false);
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
                                            apply_layout();
                                            set_layout_menu_open.set(false);
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
                                            set_rename_value.set(current.name.clone());
                                            set_renaming.set(true);
                                        }
                                        set_layout_menu_open.set(false);
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
                                        duplicate_layout();
                                        set_layout_menu_open.set(false);
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
                                        delete_layout();
                                        set_layout_menu_open.set(false);
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

            // Three-column layout
            <Show
                when=move || has_layout.get()
                fallback=move || {
                    view! {
                        <div class="flex-1 flex items-center justify-center">
                            <div class="text-center space-y-3 animate-fade-in">
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
                    class="flex min-h-0 flex-1 overflow-hidden"
                    node_ref=container_ref
                    style=move || {
                        match dragging.get() {
                            Some(PanelDrag::Sidebar) => "cursor: col-resize; user-select: none",
                            Some(PanelDrag::Bottom) => "cursor: row-resize; user-select: none",
                            None => "",
                        }
                    }
                >
                    // Left palette — resizable width
                    <div
                        class="shrink-0 min-h-0 overflow-y-auto"
                        style=move || format!("width: {:.0}px", sidebar_width.get())
                    >
                        <LayoutPalette />
                    </div>

                    // Sidebar resize handle
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

                    // Main area: canvas above, zone properties below
                    <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
                        // Canvas viewport — flexes to fill remaining space
                        <div class="relative min-h-0 flex-1 overflow-hidden">
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
        </div>
    }
}
