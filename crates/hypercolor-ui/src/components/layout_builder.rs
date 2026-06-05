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

mod editor_session;
mod library_provider;

pub(crate) use editor_session::{LayoutEditorContext, LayoutZoneDisplayContext};
use editor_session::{LayoutEditorSession, attachment_profiles_resource};
pub(crate) use library_provider::{LayoutEditorProvider, LayoutEditorState};

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

    let session = LayoutEditorSession::new(false);
    let layout = session.layout;
    let saved_layout = session.saved_layout;
    let set_saved_layout = session.set_saved_layout;
    let set_selected_zone_ids = session.set_selected_zone_ids;
    let set_hidden_zones = session.set_hidden_zones;
    let set_compound_depth = session.set_compound_depth;
    let set_layout = session.write;
    let is_dirty = session.is_dirty;

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

    session.provide_editor_context(push_preview);

    let attachment_profiles = attachment_profiles_resource(layout, devices_ctx.devices_resource);
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
