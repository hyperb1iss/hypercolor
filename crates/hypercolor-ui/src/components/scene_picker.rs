//! The scene picker: one control for the active scene, mounted in the
//! title row of every page header that depends on it.
//!
//! The trigger names the active scene. Its portaled panel lists Default
//! and every saved scene, then the actions that make, rename, and delete
//! scenes; new names and the delete confirmation happen inside the panel.
//! Everything reads [`ScenesContext`], and activation has no optimistic
//! flip: the trigger's name changes only when the shared scene resource
//! confirms the switch, while a spinner marks the scene being switched to.
//!
//! The row and action rules are pure ([`scene_rows`], [`scene_actions`]),
//! so they are testable without a DOM.

use std::sync::atomic::{AtomicU64, Ordering};

use hypercolor_leptos_ext::events::{Input, document as browser_document};
use hypercolor_leptos_ext::prelude::{
    viewport_height as browser_viewport_height, viewport_width as browser_viewport_width,
};
use leptos::portal::Portal;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;
use web_sys::KeyboardEvent;

use hypercolor_types::scene::{SceneKind, SceneMutationMode};

use crate::api;
use crate::components::control_panel::ControlDropdownDismissHandlers;
use crate::components::silk_select::next_highlight;
use crate::icons::*;
use crate::toasts;
use crate::zones::{ScenesContext, ZonesContext};

const PANEL_WIDTH: f64 = 272.0;
const PANEL_MARGIN: f64 = 12.0;
const PANEL_GAP: f64 = 6.0;

fn next_scene_picker_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// One scene in the picker list. `id` is `None` for the Default row,
/// which returns to the ephemeral default scene via deactivation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneRow {
    /// Saved scene id, or `None` for the Default row.
    pub id: Option<String>,
    /// User-facing name.
    pub label: String,
    /// Snapshot-locked scenes carry a lock glyph; still activatable.
    pub locked: bool,
    /// Whether this row is the scene currently on screen.
    pub active: bool,
}

/// An action listed after the scenes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneAction {
    /// Make a scene, by duplicating the current one or starting blank.
    New,
    /// Rename the active saved scene.
    Rename,
    /// Delete the active saved scene.
    Delete,
}

impl SceneAction {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::New => "New scene",
            Self::Rename => "Rename",
            Self::Delete => "Delete",
        }
    }
}

/// What a new scene starts from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NewSceneStart {
    /// A copy of the scene running now: its zones, layers, and layout.
    #[default]
    Duplicate,
    /// A fresh scene: one Main zone holding every output in the rig, with
    /// no layers.
    Blank,
}

/// The saved scene currently active, if any. The ephemeral default
/// reports `None`; it is represented by the Default row instead.
#[must_use]
pub fn active_saved_scene_id(active: Option<&api::SceneDocument>) -> Option<String> {
    active
        .filter(|scene| scene.kind != SceneKind::Ephemeral)
        .map(|scene| scene.id.to_string())
}

/// The name the trigger shows: the saved scene's name, or "Default"
/// while the ephemeral default is running.
#[must_use]
pub fn active_scene_label(active: Option<&api::SceneDocument>) -> String {
    active
        .filter(|scene| scene.kind != SceneKind::Ephemeral)
        .map_or_else(|| "Default".to_owned(), |scene| scene.name.clone())
}

/// Whether the active scene is snapshot-locked (the trigger's lock
/// glyph). The ephemeral default is never locked.
#[must_use]
pub fn active_scene_locked(active: Option<&api::SceneDocument>) -> bool {
    active.is_some_and(|scene| {
        scene.kind != SceneKind::Ephemeral && scene.mutation_mode == SceneMutationMode::Snapshot
    })
}

/// The picker rows: Default first, always, then every saved scene in list
/// order. An active scene the list does not know yet (a fresh activation
/// racing the list refetch) is folded in after Default so the active row
/// always exists. Exactly one row is active.
#[must_use]
pub fn scene_rows(
    scenes: &[api::SceneSummary],
    active: Option<&api::SceneDocument>,
) -> Vec<SceneRow> {
    let active_id = active_saved_scene_id(active);
    let mut rows = Vec::with_capacity(scenes.len() + 1);
    rows.push(SceneRow {
        id: None,
        label: "Default".to_owned(),
        locked: false,
        active: active_id.is_none(),
    });
    if let Some(scene) = active.filter(|scene| {
        scene.kind != SceneKind::Ephemeral
            && !scenes
                .iter()
                .any(|listed| listed.id == scene.id.to_string())
    }) {
        rows.push(SceneRow {
            id: Some(scene.id.to_string()),
            label: scene.name.clone(),
            locked: scene.mutation_mode == SceneMutationMode::Snapshot,
            active: true,
        });
    }
    rows.extend(scenes.iter().map(|scene| SceneRow {
        id: Some(scene.id.clone()),
        label: scene.name.clone(),
        locked: scene.mutation_mode == SceneMutationMode::Snapshot,
        active: active_id.as_deref() == Some(scene.id.as_str()),
    }));
    rows
}

/// The actions after the scene list. New is always offered, so a rig
/// with nothing but Default can still make its first scene. Rename and
/// Delete act on the active saved scene and are absent while Default runs.
#[must_use]
pub fn scene_actions(active: Option<&api::SceneDocument>) -> Vec<SceneAction> {
    if active_saved_scene_id(active).is_some() {
        vec![SceneAction::New, SceneAction::Rename, SceneAction::Delete]
    } else {
        vec![SceneAction::New]
    }
}

/// What the open panel shows. Rename and delete remember the scene they
/// were opened for, so a switch from another client cancels them instead
/// of acting on whatever became active.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PanelView {
    List,
    Create,
    Rename { scene_id: String },
    ConfirmDelete { scene_id: String, name: String },
}

impl PanelView {
    fn target_scene(&self) -> Option<&str> {
        match self {
            Self::Rename { scene_id } | Self::ConfirmDelete { scene_id, .. } => Some(scene_id),
            Self::List | Self::Create => None,
        }
    }
}

/// Fixed-position style for the portaled panel. The trigger sits at the
/// trailing end of the page header, so the panel's right edge lines up
/// with the trigger's and the panel grows leftward.
fn panel_style(trigger: Option<web_sys::HtmlButtonElement>) -> String {
    let Some(trigger) = trigger else {
        return String::new();
    };
    let rect = trigger.get_bounding_client_rect();
    let viewport_width = browser_viewport_width(rect.right());
    let viewport_height = browser_viewport_height(rect.bottom());
    let width = PANEL_WIDTH
        .max(rect.width())
        .min((viewport_width - 2.0 * PANEL_MARGIN).max(160.0));
    let max_left = (viewport_width - width - PANEL_MARGIN).max(PANEL_MARGIN);
    let left = (rect.right() - width).clamp(PANEL_MARGIN, max_left);
    let top = rect.bottom() + PANEL_GAP;
    let max_height = (viewport_height - top - PANEL_MARGIN).clamp(160.0, 440.0);
    format!("top: {top}px; left: {left}px; width: {width}px; max-height: {max_height}px")
}

/// The scene picker trigger and its panel.
#[component]
pub fn ScenePicker() -> impl IntoView {
    let scenes_ctx = expect_context::<ScenesContext>();
    let zones_ctx = expect_context::<ZonesContext>();
    let switching = scenes_ctx.switching;

    let (open, set_open) = signal(false);
    let panel_view = RwSignal::new(PanelView::List);
    let highlighted = RwSignal::new(None::<usize>);
    let draft = RwSignal::new(String::new());
    let new_start = RwSignal::new(NewSceneStart::default());
    let busy = RwSignal::new(false);
    let trigger_ref = NodeRef::<leptos::html::Button>::new();
    let menu_ref = NodeRef::<leptos::html::Div>::new();

    let picker_id = next_scene_picker_id();
    let marker = StoredValue::new(format!("scene-picker-{picker_id}"));
    let menu_id = StoredValue::new(format!("scene-picker-{picker_id}-menu"));
    let item_id = move |index: usize| format!("scene-picker-{picker_id}-item-{index}");

    let rows = Memo::new(move |_| {
        scenes_ctx.scenes.with(|scenes| {
            scenes_ctx
                .active
                .with(|active| scene_rows(scenes, active.as_ref()))
        })
    });
    let actions = Memo::new(move |_| {
        scenes_ctx
            .active
            .with(|active| scene_actions(active.as_ref()))
    });
    let label = Memo::new(move |_| {
        scenes_ctx
            .active
            .with(|active| active_scene_label(active.as_ref()))
    });
    let locked = Memo::new(move |_| {
        scenes_ctx
            .active
            .with(|active| active_scene_locked(active.as_ref()))
    });
    let active_saved = Memo::new(move |_| {
        scenes_ctx
            .active
            .with(|active| active_saved_scene_id(active.as_ref()))
    });
    let active_row_index =
        move || rows.with_untracked(|rows| rows.iter().position(|row| row.active));
    let item_count = move || rows.with_untracked(Vec::len) + actions.with_untracked(Vec::len);

    let focus_trigger = move || {
        if let Some(trigger) = trigger_ref.get_untracked() {
            let _ = trigger.focus();
        }
    };
    let close = move || {
        set_open.set(false);
        focus_trigger();
    };
    let back_to_list = move || {
        panel_view.set(PanelView::List);
        focus_trigger();
    };

    // Close once the activation request settles; the trigger names the
    // new scene as soon as the shared scene resource has refetched.
    Effect::new(move |previous: Option<Option<String>>| {
        let current = switching.get();
        if current.is_none() && previous.flatten().is_some() && open.get_untracked() {
            close();
        }
        current
    });

    // A closed panel always reopens on the list.
    Effect::new(move |_| {
        if open.get() {
            return;
        }
        if panel_view.with_untracked(|view| *view != PanelView::List) {
            panel_view.set(PanelView::List);
        }
        if highlighted.get_untracked().is_some() {
            highlighted.set(None);
        }
    });

    // Rename and delete act on the scene they were opened for. When a
    // different scene becomes active first, they quietly stand down.
    Effect::new(move |_| {
        let active = active_saved.get();
        let stale = panel_view.with_untracked(|view| {
            view.target_scene()
                .is_some_and(|target| active.as_deref() != Some(target))
        });
        if stale {
            panel_view.set(PanelView::List);
        }
    });

    // The list takes focus as it opens, so its keys and its active item
    // belong to the menu rather than to the trigger.
    Effect::new(move |_| {
        if open.get() && panel_view.with(|view| *view == PanelView::List) {
            request_animation_frame(move || {
                if let Some(menu) = menu_ref.get_untracked() {
                    let _ = menu.focus();
                }
            });
        }
    });

    // Keep the keyboard highlight scrolled into view. The rAF waits for
    // the portal to render the rows.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        let Some(index) = highlighted.get() else {
            return;
        };
        let id = item_id(index);
        request_animation_frame(move || {
            if let Some(doc) = browser_document()
                && let Some(el) = doc.get_element_by_id(&id)
            {
                el.scroll_into_view_with_bool(false);
            }
        });
    });

    let choose_row = move |row: SceneRow| {
        if switching.get_untracked().is_some() {
            return;
        }
        if row.active {
            close();
            return;
        }
        match row.id {
            Some(id) => scenes_ctx.activate.run(id),
            None => scenes_ctx.deactivate.run(()),
        }
    };

    let run_action = move |action: SceneAction| match action {
        SceneAction::New => {
            draft.set(String::new());
            new_start.set(NewSceneStart::Duplicate);
            panel_view.set(PanelView::Create);
        }
        SceneAction::Rename => {
            if let Some(scene_id) = active_saved.get_untracked() {
                draft.set(label.get_untracked());
                panel_view.set(PanelView::Rename { scene_id });
            }
        }
        SceneAction::Delete => {
            if let Some(scene_id) = active_saved.get_untracked() {
                panel_view.set(PanelView::ConfirmDelete {
                    scene_id,
                    name: label.get_untracked(),
                });
            }
        }
    };

    let activate_item = move |index: usize| {
        let row_count = rows.with_untracked(Vec::len);
        if index < row_count {
            if let Some(row) = rows.with_untracked(|rows| rows.get(index).cloned()) {
                choose_row(row);
            }
        } else if let Some(action) =
            actions.with_untracked(|actions| actions.get(index - row_count).copied())
        {
            run_action(action);
        }
    };

    let commit_create = move || {
        let name = draft.get_untracked().trim().to_owned();
        if name.is_empty() || busy.get_untracked() {
            return;
        }
        let start = new_start.get_untracked();
        busy.set(true);
        spawn_local(async move {
            let created = match start {
                NewSceneStart::Duplicate => api::snapshot_active_scene(&name).await,
                NewSceneStart::Blank => api::create_scene(&name).await,
            };
            let summary = match created {
                Ok(summary) => summary,
                Err(error) => {
                    let _ = busy.try_set(false);
                    toasts::toast_error(&format!("Couldn't create the scene: {error}"));
                    return;
                }
            };
            if start == NewSceneStart::Duplicate
                && let Err(error) = api::unlock_scene(&summary.id).await
            {
                // The copy exists but refuses edits; show it in the list
                // rather than switching into a scene that cannot change.
                let _ = busy.try_set(false);
                scenes_ctx.refresh.run(());
                toasts::toast_error(&format!(
                    "Created \"{name}\", but it is still snapshot-locked: {error}"
                ));
                return;
            }
            let _ = busy.try_set(false);
            toasts::toast_success("Scene created");
            let _ = panel_view.try_set(PanelView::List);
            scenes_ctx.activate.run(summary.id);
        });
    };

    let commit_rename = move || {
        let Some(scene_id) = panel_view.with_untracked(|view| match view {
            PanelView::Rename { scene_id } => Some(scene_id.clone()),
            _ => None,
        }) else {
            return;
        };
        let name = draft.get_untracked().trim().to_owned();
        if busy.get_untracked() {
            return;
        }
        if name.is_empty() || name == label.get_untracked() {
            back_to_list();
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let renamed = api::rename_scene(&scene_id, &name).await;
            let _ = busy.try_set(false);
            match renamed {
                Ok(()) => {
                    scenes_ctx.refresh.run(());
                    toasts::toast_success("Scene renamed");
                    let _ = set_open.try_set(false);
                }
                Err(error) => toasts::toast_error(&format!("Rename failed: {error}")),
            }
        });
    };

    let commit_delete = move || {
        let Some((scene_id, name)) = panel_view.with_untracked(|view| match view {
            PanelView::ConfirmDelete { scene_id, name } => Some((scene_id.clone(), name.clone())),
            _ => None,
        }) else {
            return;
        };
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let deleted = api::delete_scene(&scene_id).await;
            let _ = busy.try_set(false);
            match deleted {
                Ok(()) => {
                    scenes_ctx.refresh.run(());
                    toasts::toast_info(&format!("Deleted {name}"));
                    let _ = set_open.try_set(false);
                }
                Err(error) => toasts::toast_error(&format!("Delete failed: {error}")),
            }
        });
    };

    let on_trigger_keydown = move |ev: KeyboardEvent| {
        let key = ev.key();
        match key.as_str() {
            "Escape" if open.get_untracked() => {
                ev.prevent_default();
                set_open.set(false);
            }
            "ArrowDown" | "ArrowUp" | "Home" | "End" if !open.get_untracked() => {
                ev.prevent_default();
                set_open.set(true);
                highlighted.set(active_row_index().or(Some(0)));
            }
            _ => {}
        }
    };

    // Keys on the open list. Escape belongs to the panel, which closes
    // the list or backs out of a form.
    let on_menu_keydown = move |ev: KeyboardEvent| {
        let key = ev.key();
        match key.as_str() {
            "ArrowDown" | "ArrowUp" | "Home" | "End" => {
                ev.prevent_default();
                highlighted.set(next_highlight(
                    highlighted.get_untracked(),
                    item_count(),
                    &key,
                ));
            }
            "Enter" | " " => {
                ev.prevent_default();
                match highlighted.get_untracked() {
                    Some(index) => activate_item(index),
                    None => close(),
                }
            }
            "Tab" => {
                ev.prevent_default();
                close();
            }
            _ => {}
        }
    };

    let on_panel_keydown = move |ev: KeyboardEvent| {
        if ev.key() != "Escape" {
            return;
        }
        ev.prevent_default();
        ev.stop_propagation();
        if panel_view.with_untracked(|view| *view == PanelView::List) {
            close();
        } else {
            back_to_list();
        }
    };

    view! {
        <div class=move || format!("relative min-w-0 {}", marker.get_value())>
            <button
                type="button"
                node_ref=trigger_ref
                class="group flex min-w-0 max-w-full items-center gap-2 rounded-lg border \
                       border-edge-subtle bg-surface-sunken px-2.5 py-1.5 text-left \
                       transition-colors duration-150 hover:border-edge-strong btn-press"
                class=("border-accent-muted", move || open.get())
                aria-haspopup="menu"
                aria-expanded=move || open.get().to_string()
                aria-controls=move || {
                    (open.get() && panel_view.with(|view| *view == PanelView::List))
                        .then(|| menu_id.get_value())
                }
                title="Switch scene"
                on:click=move |_| {
                    set_open.update(|value| *value = !*value);
                    if open.get_untracked() && highlighted.get_untracked().is_none() {
                        highlighted.set(active_row_index());
                    }
                }
                on:keydown=on_trigger_keydown
            >
                <span class="max-md:hidden shrink-0 text-[10px] font-semibold uppercase tracking-[0.14em] text-fg-tertiary">
                    "Scene"
                </span>
                {move || {
                    if zones_ctx.scene_resolved.get() {
                        view! {
                            <span class="min-w-0 max-w-[200px] max-md:max-w-[120px] truncate text-[13px] font-medium text-fg-primary">
                                {label.get()}
                            </span>
                        }
                            .into_any()
                    } else {
                        view! {
                            <span class="h-3 w-16 shrink-0 rounded bg-surface-hover" aria-hidden="true" />
                        }
                            .into_any()
                    }
                }}
                {move || locked.get().then(|| view! {
                    <span class="flex shrink-0 text-status-warning" title="Snapshot-locked scene">
                        <Icon icon=LuLock width="11px" height="11px" />
                        <span class="sr-only">"Snapshot-locked"</span>
                    </span>
                })}
                {move || switching.get().is_some().then(|| view! {
                    <span class="flex shrink-0 animate-spin text-fg-tertiary">
                        <Icon icon=LuLoader width="12px" height="12px" />
                        <span class="sr-only">"Switching scene"</span>
                    </span>
                })}
                <span
                    class="flex shrink-0 text-fg-tertiary transition-transform duration-200 group-hover:text-fg-secondary"
                    class=("rotate-180", move || open.get())
                >
                    <Icon icon=LuChevronDown width="12px" height="12px" />
                </span>
            </button>

            <Show when=move || open.get()>
                <ControlDropdownDismissHandlers
                    class_name=marker.get_value()
                    is_open=open
                    set_open=set_open
                />
                <Portal>
                    <div class=move || marker.get_value()>
                        <div
                            class="fixed z-[9999] flex flex-col overflow-hidden rounded-xl border \
                                   border-edge-subtle bg-surface-overlay/98 backdrop-blur-xl \
                                   dropdown-glow animate-enter-down"
                            style=move || panel_style(trigger_ref.get())
                            on:pointerdown=|ev: leptos::ev::PointerEvent| ev.stop_propagation()
                            on:keydown=on_panel_keydown
                        >
                            {move || match panel_view.get() {
                                PanelView::List => view! {
                                    <SceneList
                                        rows=rows
                                        actions=actions
                                        highlighted=highlighted
                                        menu_id=menu_id.get_value()
                                        menu_ref=menu_ref
                                        on_keydown=Callback::new(on_menu_keydown)
                                        item_id=Callback::new(item_id)
                                        on_choose=Callback::new(choose_row)
                                        on_action=Callback::new(run_action)
                                    />
                                }
                                    .into_any(),
                                PanelView::Create => view! {
                                    <NameForm
                                        title="New scene"
                                        submit_label="Create scene"
                                        draft=draft
                                        busy=busy
                                        on_submit=Callback::new(move |()| commit_create())
                                        on_cancel=Callback::new(move |()| back_to_list())
                                    >
                                        <StartChoice start=new_start />
                                    </NameForm>
                                }
                                    .into_any(),
                                PanelView::Rename { .. } => view! {
                                    <NameForm
                                        title="Rename scene"
                                        submit_label="Save name"
                                        draft=draft
                                        busy=busy
                                        on_submit=Callback::new(move |()| commit_rename())
                                        on_cancel=Callback::new(move |()| back_to_list())
                                    />
                                }
                                    .into_any(),
                                PanelView::ConfirmDelete { name, .. } => view! {
                                    <DeleteConfirm
                                        name=name
                                        busy=busy
                                        on_delete=Callback::new(move |()| commit_delete())
                                        on_keep=Callback::new(move |()| back_to_list())
                                    />
                                }
                                    .into_any(),
                            }}
                        </div>
                    </div>
                </Portal>
            </Show>
        </div>
    }
}

/// One menu holding the scene rows (radio items, the active scene
/// checked) and, after a divider, the actions, each group labelled. The
/// menu holds focus while open and names the highlighted item through
/// `aria-activedescendant`, so every item carries an id.
#[component]
fn SceneList(
    rows: Memo<Vec<SceneRow>>,
    actions: Memo<Vec<SceneAction>>,
    highlighted: RwSignal<Option<usize>>,
    menu_id: String,
    menu_ref: NodeRef<leptos::html::Div>,
    on_keydown: Callback<KeyboardEvent>,
    item_id: Callback<usize, String>,
    on_choose: Callback<SceneRow>,
    on_action: Callback<SceneAction>,
) -> impl IntoView {
    let scenes_ctx = expect_context::<ScenesContext>();
    let switching = scenes_ctx.switching;

    view! {
        <div
            id=menu_id
            node_ref=menu_ref
            role="menu"
            aria-label="Scene picker"
            tabindex="-1"
            aria-activedescendant=move || highlighted.get().map(|index| item_id.run(index))
            class="flex min-h-0 flex-1 flex-col outline-none"
            on:keydown=move |ev| on_keydown.run(ev)
        >
        <div
            role="group"
            aria-label="Scenes"
            class="min-h-0 flex-1 overflow-y-auto py-1 scrollbar-dropdown"
        >
            {move || {
                rows.get()
                    .into_iter()
                    .enumerate()
                    .map(|(index, row)| {
                        // The deactivate path reports `switching == Some("")`,
                        // so the Default row's empty token matches it.
                        let token = row.id.clone().unwrap_or_default();
                        let is_switching =
                            move || switching.get().as_deref() == Some(token.as_str());
                        let is_active = row.active;
                        let row_label = row.label.clone();
                        let locked = row.locked;
                        view! {
                            <button
                                type="button"
                                id=item_id.run(index)
                                role="menuitemradio"
                                aria-checked=is_active.to_string()
                                tabindex="-1"
                                class="dropdown-option flex w-full items-center gap-2 px-3 py-[7px] \
                                       text-left text-xs text-fg-secondary \
                                       disabled:cursor-not-allowed disabled:opacity-50"
                                class=("dropdown-option-active", is_active)
                                class=("dropdown-option-keyboard", move || highlighted.get() == Some(index))
                                disabled=move || switching.get().is_some()
                                on:click=move |_| on_choose.run(row.clone())
                            >
                                <span class="flex h-3.5 w-3.5 shrink-0 items-center justify-center">
                                    {move || {
                                        if is_switching() {
                                            Some(view! {
                                                <span class="flex animate-spin text-fg-tertiary">
                                                    <Icon icon=LuLoader width="12px" height="12px" />
                                                    <span class="sr-only">"Switching"</span>
                                                </span>
                                            }.into_any())
                                        } else if is_active {
                                            Some(view! {
                                                <span class="flex text-accent">
                                                    <Icon icon=LuCheck width="12px" height="12px" />
                                                </span>
                                            }.into_any())
                                        } else {
                                            None
                                        }
                                    }}
                                </span>
                                <span class="min-w-0 flex-1 truncate" class:text-fg-primary=is_active>
                                    {row_label}
                                </span>
                                {locked.then(|| view! {
                                    <span class="flex shrink-0 text-status-warning" title="Snapshot-locked scene">
                                        <Icon icon=LuLock width="11px" height="11px" />
                                        <span class="sr-only">"Snapshot-locked"</span>
                                    </span>
                                })}
                            </button>
                        }
                    })
                    .collect_view()
            }}
        </div>
        <div class="mx-2 h-px shrink-0 bg-edge-subtle/60" aria-hidden="true" />
        <div class="shrink-0 py-1" role="group" aria-label="Scene actions">
            {move || {
                let row_count = rows.with(Vec::len);
                actions
                    .get()
                    .into_iter()
                    .enumerate()
                    .map(|(offset, action)| {
                        let index = row_count + offset;
                        let icon = match action {
                            SceneAction::New => LuPlus,
                            SceneAction::Rename => LuPencil,
                            SceneAction::Delete => LuTrash2,
                        };
                        let tone = if action == SceneAction::Delete {
                            "text-status-error/80 hover:text-status-error"
                        } else {
                            "text-fg-secondary"
                        };
                        view! {
                            <button
                                type="button"
                                id=item_id.run(index)
                                role="menuitem"
                                tabindex="-1"
                                class=format!(
                                    "dropdown-option flex w-full items-center gap-2 px-3 py-[7px] text-left text-xs {tone}"
                                )
                                class=("dropdown-option-keyboard", move || highlighted.get() == Some(index))
                                on:click=move |_| on_action.run(action)
                            >
                                <span class="flex h-3.5 w-3.5 shrink-0 items-center justify-center opacity-80">
                                    <Icon icon=icon width="12px" height="12px" />
                                </span>
                                <span class="min-w-0 flex-1 truncate">{action.label()}</span>
                            </button>
                        }
                    })
                    .collect_view()
            }}
        </div>
        </div>
    }
}

/// An inline name field with Cancel and a submit button, used by New and
/// Rename. Enter submits through the form; Escape is the panel's.
#[component]
fn NameForm(
    title: &'static str,
    submit_label: &'static str,
    draft: RwSignal<String>,
    busy: RwSignal<bool>,
    on_submit: Callback<()>,
    on_cancel: Callback<()>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let input_ref = NodeRef::<leptos::html::Input>::new();
    Effect::new(move |_| {
        if let Some(input) = input_ref.get() {
            let _ = input.focus();
            input.select();
        }
    });
    let empty = move || draft.with(|value| value.trim().is_empty());

    view! {
        <form
            class="flex flex-col gap-2.5 p-3"
            on:submit=move |ev: leptos::ev::SubmitEvent| {
                ev.prevent_default();
                on_submit.run(());
            }
        >
            <div class="text-[10px] font-semibold uppercase tracking-[0.14em] text-fg-tertiary">
                {title}
            </div>
            {children.map(|children| children())}
            <input
                type="text"
                node_ref=input_ref
                aria-label="Scene name"
                placeholder="Scene name"
                class="w-full rounded-lg border border-edge-subtle bg-surface-sunken px-3 py-1.5 \
                       text-sm text-fg-primary placeholder-fg-tertiary transition-colors \
                       focus:border-accent-muted focus:outline-none glow-ring"
                prop:value=move || draft.get()
                on:input=move |ev| {
                    if let Some(value) = Input::from_event(ev).value_string() {
                        draft.set(value);
                    }
                }
            />
            <div class="flex items-center justify-end gap-2">
                <button
                    type="button"
                    class="rounded-lg px-2.5 py-1.5 text-xs text-fg-secondary transition-colors \
                           hover:bg-surface-hover hover:text-fg-primary btn-press"
                    on:click=move |_| on_cancel.run(())
                >
                    "Cancel"
                </button>
                <button
                    type="submit"
                    class="rounded-lg border border-accent-muted/40 bg-accent/15 px-3 py-1.5 text-xs \
                           font-semibold text-accent transition-colors hover:bg-accent/25 btn-press \
                           disabled:cursor-not-allowed disabled:opacity-50"
                    disabled=move || busy.get() || empty()
                >
                    {submit_label}
                </button>
            </div>
        </form>
    }
}

/// The two starting points for a new scene. Duplicate is the default:
/// a copy keeps the current zone split and layout.
#[component]
fn StartChoice(start: RwSignal<NewSceneStart>) -> impl IntoView {
    let option = move |value: NewSceneStart, text: &'static str| {
        view! {
            <button
                type="button"
                aria-pressed=move || (start.get() == value).to_string()
                class="rounded-md px-2 py-1 text-[11px] font-medium transition-colors"
                class=("bg-accent/15", move || start.get() == value)
                class=("text-fg-primary", move || start.get() == value)
                class=("text-fg-tertiary", move || start.get() != value)
                class=("hover:text-fg-secondary", move || start.get() != value)
                on:click=move |_| start.set(value)
            >
                {text}
            </button>
        }
    };
    view! {
        <div class="flex flex-col gap-1.5">
            <div
                class="grid grid-cols-2 gap-0.5 rounded-lg border border-edge-subtle bg-surface-sunken p-0.5"
                role="group"
                aria-label="Start from"
            >
                {option(NewSceneStart::Duplicate, "Duplicate current")}
                {option(NewSceneStart::Blank, "Blank")}
            </div>
            <p class="text-[11px] leading-snug text-fg-tertiary">
                {move || match start.get() {
                    NewSceneStart::Duplicate => "Copies the zones, layers, and layout running now.",
                    NewSceneStart::Blank => "One Main zone with every device in the rig, and no effects yet.",
                }}
            </p>
        </div>
    }
}

/// The one confirmation a delete asks for, naming the scene, with
/// buttons labelled by outcome. Focus starts on the safe choice.
#[component]
fn DeleteConfirm(
    name: String,
    busy: RwSignal<bool>,
    on_delete: Callback<()>,
    on_keep: Callback<()>,
) -> impl IntoView {
    let keep_ref = NodeRef::<leptos::html::Button>::new();
    Effect::new(move |_| {
        if let Some(button) = keep_ref.get() {
            let _ = button.focus();
        }
    });

    view! {
        <div class="flex flex-col gap-3 p-3" role="group" aria-label="Delete scene">
            <div class="space-y-1">
                <p class="text-xs font-medium text-fg-primary break-words">
                    {format!("Delete \"{name}\"?")}
                </p>
                <p class="text-[11px] leading-snug text-fg-tertiary">
                    "Its zones and layers go with it. This can't be undone."
                </p>
            </div>
            <div class="flex items-center justify-end gap-2">
                <button
                    type="button"
                    node_ref=keep_ref
                    class="rounded-lg px-2.5 py-1.5 text-xs text-fg-secondary transition-colors \
                           hover:bg-surface-hover hover:text-fg-primary btn-press"
                    on:click=move |_| on_keep.run(())
                >
                    "Keep scene"
                </button>
                <button
                    type="button"
                    class="rounded-lg border border-status-error/35 bg-status-error/12 px-3 py-1.5 \
                           text-xs font-semibold text-status-error transition-colors \
                           hover:bg-status-error/20 btn-press disabled:cursor-not-allowed disabled:opacity-50"
                    disabled=move || busy.get()
                    on:click=move |_| on_delete.run(())
                >
                    "Delete scene"
                </button>
            </div>
        </div>
    }
}
