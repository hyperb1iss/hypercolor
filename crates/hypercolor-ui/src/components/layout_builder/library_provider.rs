use leptos::prelude::*;
use leptos_use::use_debounce_fn_with_arg;

use crate::api;
use crate::app::DevicesContext;
use crate::layout_geometry;
use crate::layout_page_state::{LayoutPageState, PerLayoutState};
use crate::toasts;
use hypercolor_types::spatial::SpatialLayout;

use super::LayoutWriteHandle;
use super::editor_session::{
    LayoutEditorSession, LayoutZoneDisplayContext, attachment_profiles_resource,
};

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
    let session = LayoutEditorSession::new(initial_state.keep_aspect_ratio);
    let layout = session.layout;
    let saved_layout = session.saved_layout;
    let set_saved_layout = session.set_saved_layout;
    let selected_zone_ids = session.selected_zone_ids;
    let set_selected_zone_ids = session.set_selected_zone_ids;
    let compound_depth = session.compound_depth;
    let set_compound_depth = session.set_compound_depth;
    let keep_aspect_ratio = session.keep_aspect_ratio;
    let hidden_zones = session.hidden_zones;
    let set_hidden_zones = session.set_hidden_zones;
    let set_layout = session.write;
    let layout_signal = session.layout_signal;
    let can_undo = session.can_undo;
    let can_redo = session.can_redo;
    let is_dirty = session.is_dirty;
    let (creating, set_creating) = signal(false);
    let (new_layout_name, set_new_layout_name) = signal(String::new());
    let (renaming, set_renaming) = signal(false);
    let (layout_menu_open, set_layout_menu_open) = signal(false);
    let (rename_value, set_rename_value) = signal(String::new());
    let (initialized, set_initialized) = signal(false);

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

    session.provide_editor_context(push_preview);

    let attachment_profiles = attachment_profiles_resource(layout, ctx.devices_resource);
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
