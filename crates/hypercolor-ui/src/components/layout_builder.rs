//! Layout builder wrapper — toolbar + three-column layout editor.
//!
//! Edits are pushed to the spatial engine immediately for live preview.
//! Save persists to disk. Revert restores to the last saved state.

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::use_debounce_fn_with_arg;
use wasm_bindgen::JsCast;

use crate::api;
use crate::app::DevicesContext;
use crate::components::layout_canvas::LayoutCanvas;
use crate::components::layout_palette::LayoutPalette;
use crate::components::layout_zone_properties::LayoutZoneProperties;
use crate::icons::*;
use crate::layout_geometry;
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

/// Cache of removed zones so re-adding restores previous settings.
#[derive(Clone)]
pub(crate) struct RemovedZoneCache {
    pub cache: Signal<std::collections::HashMap<(String, Option<String>), DeviceZone>>,
    pub set_cache: WriteSignal<std::collections::HashMap<(String, Option<String>), DeviceZone>>,
}

/// Layout builder — wraps toolbar, palette, canvas viewport, and zone properties.
#[component]
pub fn LayoutBuilder() -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();

    let (selected_layout_id, set_selected_layout_id) = signal(None::<String>);
    let (layout, set_layout) = signal(None::<SpatialLayout>);
    let (saved_layout, set_saved_layout) = signal(None::<SpatialLayout>);
    let (selected_zone_id, set_selected_zone_id) = signal(None::<String>);
    let (creating, set_creating) = signal(false);
    let (new_layout_name, set_new_layout_name) = signal(String::new());
    let (initialized, set_initialized) = signal(false);
    let (keep_aspect_ratio, set_keep_aspect_ratio) = signal(true);
    let (hidden_zones, set_hidden_zones) =
        signal(std::collections::HashSet::<String>::new());

    // Removed-zone cache: zones stashed on remove so re-adding restores settings.
    // Shared via context between palette and zone properties.
    let (removed_zone_cache, set_removed_zone_cache) = signal(
        std::collections::HashMap::<(String, Option<String>), DeviceZone>::new(),
    );
    provide_context(RemovedZoneCache {
        cache: removed_zone_cache.into(),
        set_cache: set_removed_zone_cache,
    });
    let layout_signal = Signal::derive(move || layout.get());
    let zone_id_signal = Signal::derive(move || selected_zone_id.get());
    let keep_aspect_ratio_signal = Signal::derive(move || keep_aspect_ratio.get());
    let hidden_zones_signal = Signal::derive(move || hidden_zones.get());
    let has_layout = Signal::derive(move || layout.with(|current| current.is_some()));

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

    // Derive dirty state by comparing working layout to saved snapshot.
    let is_dirty = Signal::derive(move || {
        let current = layout.get();
        let saved = saved_layout.get();
        match (current, saved) {
            (Some(c), Some(s)) => c.zones != s.zones || c.groups != s.groups,
            _ => false,
        }
    });

    // Child components still expect a writable dirty signal even though the
    // actual dirty state is derived from layout vs saved_layout comparison.
    let (_dirty_marker, set_is_dirty) = signal(false);
    let preview_layout = use_debounce_fn_with_arg(
        |layout: SpatialLayout| {
            leptos::task::spawn_local(async move {
                let _ = api::preview_layout(&layout).await;
            });
        },
        75.0,
    );

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

    // Load layout when selection changes
    Effect::new(move |_| {
        let id = selected_layout_id.get();
        if let Some(id) = id {
            let set_layout = set_layout;
            let set_saved = set_saved_layout;
            leptos::task::spawn_local(async move {
                if let Ok(l) = api::fetch_layout(&id).await {
                    let mut repaired_layout = l.clone();
                    let repaired_legacy_lcd =
                        layout_geometry::repair_legacy_lcd_defaults(&mut repaired_layout);
                    let layout = layout_geometry::normalize_layout_for_editor(repaired_layout);
                    let saved_layout = if repaired_legacy_lcd {
                        layout_geometry::normalize_layout_for_editor(l)
                    } else {
                        layout.clone()
                    };
                    set_saved.set(Some(saved_layout));
                    set_layout.set(Some(layout));
                }
            });
        } else {
            set_layout.set(None);
            set_saved_layout.set(None);
        }
        set_selected_zone_id.set(None);
    });

    // Push live preview to spatial engine whenever the layout changes (debounced).
    Effect::new(
        move |prev_snapshot: Option<
            Option<(
                Vec<hypercolor_types::spatial::DeviceZone>,
                Vec<hypercolor_types::spatial::ZoneGroup>,
            )>,
        >| {
            let current = layout.get();
            let current_snapshot = current
                .as_ref()
                .map(|current| (current.zones.clone(), current.groups.clone()));

            // Only push preview if spatial data actually changed (avoid initial no-op).
            if current_snapshot != prev_snapshot.flatten() {
                if let Some(layout) = current.as_ref() {
                    preview_layout(layout.clone());
                }
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
        let groups = l.groups.clone();
        let saved_copy = l.clone();
        let layouts_resource = ctx.layouts_resource;
        leptos::task::spawn_local(async move {
            let req = api::UpdateLayoutApiRequest {
                name: None,
                description: None,
                canvas_width: None,
                canvas_height: None,
                zones: Some(zones),
                groups: Some(groups),
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
        set_layout.set(Some(saved));
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

    // Duplicate handler — creates a copy of the current layout with zones + groups
    let duplicate_layout = move || {
        let Some(current) = layout.get_untracked() else {
            return;
        };
        let new_name = format!("{} (copy)", current.name);
        let zones = current.zones.clone();
        let groups = current.groups.clone();
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
                    // Update the new layout with zones + groups from the original
                    let update_req = api::UpdateLayoutApiRequest {
                        name: None,
                        description: None,
                        canvas_width: None,
                        canvas_height: None,
                        zones: Some(zones),
                        groups: Some(groups),
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
            // Toolbar — glass background with edge glow
            <div class="shrink-0 px-5 py-2.5 flex items-center gap-3 glass-subtle border-b border-edge-subtle">
                // Layout selector
                <div class="flex items-center gap-3">
                    <h1 class="text-lg font-medium text-fg-primary">"Layout"</h1>
                    <Suspense fallback=|| ()>
                        {move || {
                            ctx.layouts_resource.get().map(|result| {
                                let layouts = result.unwrap_or_default();
                                view! {
                                    <select
                                        class="bg-surface-sunken border border-edge-subtle rounded-lg px-3 py-1.5 text-sm text-fg-primary
                                               focus:outline-none focus:border-accent-muted glow-ring min-w-[180px] transition-all"
                                        on:change=move |ev| {
                                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
                                            if let Some(el) = target {
                                                let val = el.value();
                                                if val.is_empty() {
                                                    set_selected_layout_id.set(None);
                                                } else {
                                                    set_selected_layout_id.set(Some(val));
                                                }
                                            }
                                        }
                                    >
                                        <option value="" selected=move || selected_layout_id.get().is_none()>"Select layout..."</option>
                                        {layouts.into_iter().map(|l| {
                                            let lid = l.id.clone();
                                            let lid2 = l.id.clone();
                                            let label = if l.is_active {
                                                format!("{} ({} zones) *", l.name, l.zone_count)
                                            } else {
                                                format!("{} ({} zones)", l.name, l.zone_count)
                                            };
                                            view! {
                                                <option
                                                    value=lid
                                                    selected=move || selected_layout_id.get().as_deref() == Some(&lid2)
                                                >
                                                    {label}
                                                </option>
                                            }
                                        }).collect_view()}
                                    </select>
                                }
                            })
                        }}
                    </Suspense>

                    // Dirty indicator
                    <Show when=move || is_dirty.get()>
                        <div class="w-2 h-2 rounded-full bg-electric-yellow dot-alive" title="Unsaved changes" />
                    </Show>
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
                                    let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                    if let Some(el) = target { set_new_layout_name.set(el.value()); }
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

                // Duplicate button — only when a layout is loaded
                {move || layout.get().map(|_| {
                    view! {
                        <button
                            class="flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-medium border whitespace-nowrap transition-all btn-press"
                            style="background: rgba(128, 255, 234, 0.06); border-color: rgba(128, 255, 234, 0.15); color: rgba(128, 255, 234, 0.8)"
                            title="Duplicate current layout"
                            on:click=move |_| duplicate_layout()
                        >
                            <Icon icon=LuCopy width="12px" height="12px" />
                            "Duplicate"
                        </button>
                    }
                })}

                <div class="flex-1" />

                // Save / Revert / Delete buttons — only when a layout is loaded
                {move || layout.get().map(|_| {
                    let dirty = is_dirty.get();
                    let is_active = selected_layout_is_active.get();
                    let apply_style = if is_active || dirty {
                        "background: var(--color-surface-overlay); border-color: var(--color-border-subtle); color: var(--color-text-tertiary); opacity: 0.4; pointer-events: none"
                    } else {
                        "background: rgba(128, 255, 234, 0.1); border-color: rgba(128, 255, 234, 0.2); color: rgb(128, 255, 234)"
                    };
                    let save_style = if dirty {
                        "background: rgba(80, 250, 123, 0.12); border-color: rgba(80, 250, 123, 0.3); color: rgb(80, 250, 123); \
                         box-shadow: 0 0 12px rgba(80, 250, 123, 0.15)"
                    } else {
                        "background: var(--color-surface-overlay); border-color: var(--color-border-subtle); color: var(--color-text-tertiary); opacity: 0.4; pointer-events: none"
                    };
                    let revert_style = if dirty {
                        "background: rgba(241, 250, 140, 0.08); border-color: rgba(241, 250, 140, 0.2); color: rgb(241, 250, 140)"
                    } else {
                        "background: var(--color-surface-overlay); border-color: var(--color-border-subtle); color: var(--color-text-tertiary); opacity: 0.4; pointer-events: none"
                    };
                    view! {
                        <div class="flex items-center gap-2">
                            <button
                                class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium border transition-all btn-press"
                                style=apply_style
                                on:click=move |_| apply_layout()
                                disabled=move || is_dirty.get() || selected_layout_is_active.get()
                            >
                                <Icon icon=LuCheck width="14px" height="14px" />
                                {move || if selected_layout_is_active.get() { "Active" } else { "Apply" }}
                            </button>
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
                            <button
                                class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium border transition-all btn-press
                                       text-status-error/40 hover:text-status-error"
                                style="background: rgba(255, 99, 99, 0.04); border-color: rgba(255, 99, 99, 0.12)"
                                on:click=move |_| delete_layout()
                            >
                                <Icon icon=LuTrash2 width="14px" height="14px" />
                                "Delete"
                            </button>
                        </div>
                    }
                })}
            </div>

            // Three-column layout
            <Show
                when=move || has_layout.get()
                fallback=move || {
                    view! {
                        <div class="flex-1 flex items-center justify-center">
                            <div class="text-center space-y-3 animate-fade-in">
                                <Icon icon=LuLayoutTemplate width="48px" height="48px" style="color: rgba(139, 133, 160, 0.15)" />
                                <div class="text-fg-tertiary text-sm">"Select or create a layout to begin"</div>
                                <div class="text-fg-tertiary/40 text-xs">"Drag devices onto the canvas to build your spatial mapping"</div>
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
                        <LayoutPalette
                            layout=layout_signal
                            selected_zone_id=zone_id_signal
                            hidden_zones=hidden_zones_signal
                            set_layout=set_layout
                            set_selected_zone_id=set_selected_zone_id
                            set_is_dirty=set_is_dirty
                            set_hidden_zones=set_hidden_zones
                        />
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
                            <LayoutCanvas
                                layout=layout_signal
                                selected_zone_id=zone_id_signal
                                keep_aspect_ratio=keep_aspect_ratio_signal
                                hidden_zones=hidden_zones_signal
                                set_selected_zone_id=set_selected_zone_id
                                set_layout=set_layout
                                set_is_dirty=set_is_dirty
                            />
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
                            <LayoutZoneProperties
                                layout=layout_signal
                                selected_zone_id=zone_id_signal
                                keep_aspect_ratio=keep_aspect_ratio_signal
                                set_layout=set_layout
                                set_keep_aspect_ratio=set_keep_aspect_ratio
                                set_selected_zone_id=set_selected_zone_id
                                set_is_dirty=set_is_dirty
                            />
                        </div>
                    </div>
                </div>
            </Show>
        </div>
    }
}
