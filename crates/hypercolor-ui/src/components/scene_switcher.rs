//! Shared scene switcher — one popover used by the sidebar scene chip,
//! the dashboard status pill, and the Studio scene selector.
//!
//! The popover lists a "Default" row (mapped to scene deactivation)
//! followed by every saved scene. Activation routes through
//! [`ScenesContext`]: there is no optimistic flip — the trigger's label
//! changes only when the shared scene resource confirms the switch, and
//! the row being switched to shows a spinner while `switching` matches.
//!
//! The row-building logic is pure ([`scene_rows`] and friends) so the
//! presentation rules — fold-in of an unlisted active scene, the lock
//! glyph, exactly-one-active — are testable without a DOM.

use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_types::scene::{SceneKind, SceneMutationMode};

use crate::api;
use crate::components::control_panel::ControlDropdownDismissHandlers;
use crate::icons::*;
use crate::zones::ScenesContext;

/// One row of the switcher list. `id` is `None` for the Default row,
/// which returns to the ephemeral default scene via deactivation.
#[derive(Debug, Clone, PartialEq)]
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

/// The saved scene currently active, if any. The ephemeral default
/// reports `None` — it is represented by the Default row instead.
#[must_use]
pub fn active_saved_scene_id(active: Option<&api::ActiveSceneResponse>) -> Option<&str> {
    active
        .filter(|scene| scene.kind != SceneKind::Ephemeral)
        .map(|scene| scene.id.as_str())
}

/// Label switcher triggers show for the active scene: the saved scene's
/// name, or "Default" while the ephemeral default is running.
#[must_use]
pub fn active_scene_label(active: Option<&api::ActiveSceneResponse>) -> String {
    active
        .filter(|scene| scene.kind != SceneKind::Ephemeral)
        .map_or_else(|| "Default".to_owned(), |scene| scene.name.clone())
}

/// Whether the active scene is snapshot-locked (the lock glyph on
/// triggers). The ephemeral default is never locked.
#[must_use]
pub fn active_scene_locked(active: Option<&api::ActiveSceneResponse>) -> bool {
    active.is_some_and(|scene| {
        scene.kind != SceneKind::Ephemeral && scene.mutation_mode == SceneMutationMode::Snapshot
    })
}

/// Build the switcher rows: the Default row first, then every saved
/// scene in list order. An active scene the list doesn't know yet (a
/// fresh activation racing the list refetch) is folded in after the
/// Default row so the active row always exists. Exactly one row is
/// active.
#[must_use]
pub fn scene_rows(
    scenes: &[api::SceneSummary],
    active: Option<&api::ActiveSceneResponse>,
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
        scene.kind != SceneKind::Ephemeral && !scenes.iter().any(|listed| listed.id == scene.id)
    }) {
        rows.push(SceneRow {
            id: Some(scene.id.clone()),
            label: scene.name.clone(),
            locked: scene.mutation_mode == SceneMutationMode::Snapshot,
            active: true,
        });
    }
    rows.extend(scenes.iter().map(|scene| SceneRow {
        id: Some(scene.id.clone()),
        label: scene.name.clone(),
        locked: scene.mutation_mode == SceneMutationMode::Snapshot,
        active: active_id == Some(scene.id.as_str()),
    }));
    rows
}

/// Popover panel listing the scene rows. The caller owns the open state
/// and renders the trigger inside a `relative` anchor element that also
/// carries `anchor_class` (unique per call site), so the outside-click
/// dismiss handler recognises clicks inside the anchor.
#[component]
pub fn SceneSwitcherMenu(
    /// Marker class on the anchor wrapper — must be unique per call site.
    anchor_class: &'static str,
    is_open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    /// Positioning + sizing classes for the panel, relative to the anchor.
    #[prop(default = "left-0 top-full mt-1 w-52")]
    placement: &'static str,
) -> impl IntoView {
    let scenes_ctx = expect_context::<ScenesContext>();

    let rows = Memo::new(move |_| {
        scenes_ctx.scenes.with(|scenes| {
            scenes_ctx
                .active
                .with(|active| scene_rows(scenes, active.as_ref()))
        })
    });

    // Close once a switch completes so the trigger's updated label is
    // visible — the row spinner has done its job by then.
    Effect::new(move |previous: Option<Option<String>>| {
        let current = scenes_ctx.switching.get();
        if current.is_none() && previous.flatten().is_some() {
            set_open.set(false);
        }
        current
    });

    view! {
        <Show when=move || is_open.get()>
            <ControlDropdownDismissHandlers
                class_name=anchor_class.to_string()
                is_open=is_open
                set_open=set_open
            />
            <div
                class=format!(
                    "absolute z-[100] overflow-hidden rounded-lg border border-edge-subtle \
                     bg-surface-overlay/98 backdrop-blur-xl dropdown-glow animate-enter-down {placement}"
                )
                role="menu"
                aria-label="Switch scene"
                on:keydown=move |ev: web_sys::KeyboardEvent| {
                    if ev.key() == "Escape" {
                        set_open.set(false);
                    }
                }
            >
                {move || {
                    rows.get()
                        .into_iter()
                        .map(|row| view! { <SceneSwitcherRow row=row set_open=set_open /> })
                        .collect_view()
                }}
            </div>
        </Show>
    }
}

/// One scene row: leading check (active) or spinner (switching), the
/// scene name, and a lock glyph for snapshot-locked scenes.
#[component]
fn SceneSwitcherRow(row: SceneRow, set_open: WriteSignal<bool>) -> impl IntoView {
    let scenes_ctx = expect_context::<ScenesContext>();
    let switching = scenes_ctx.switching;

    // The deactivate path reports `switching == Some("")`, so the
    // Default row's empty token matches it naturally.
    let switch_token = row.id.clone().unwrap_or_default();
    let is_switching = {
        let token = switch_token.clone();
        move || switching.get().as_deref() == Some(token.as_str())
    };
    let leading_token = switch_token;
    let is_active = row.active;
    let row_id = row.id.clone();

    view! {
        <button
            type="button"
            class="dropdown-option flex w-full items-center gap-2 px-3 py-2 text-left text-xs \
                   text-fg-secondary transition-colors hover:text-fg-primary \
                   focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent/50 \
                   disabled:opacity-50 disabled:cursor-not-allowed"
            role="menuitem"
            disabled=move || switching.get().is_some()
            on:click=move |_| {
                if is_active {
                    set_open.set(false);
                    return;
                }
                match row_id.clone() {
                    Some(id) => scenes_ctx.activate.run(id),
                    None => scenes_ctx.deactivate.run(()),
                }
            }
        >
            <span class="w-3.5 h-3.5 flex items-center justify-center shrink-0">
                {move || {
                    if switching.get().as_deref() == Some(leading_token.as_str()) {
                        Some(view! {
                            <span class="flex animate-spin text-fg-tertiary">
                                <Icon icon=LuLoader width="12px" height="12px" />
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
            <span class="flex-1 min-w-0 truncate" class:text-fg-primary=is_active>
                {row.label.clone()}
            </span>
            {row.locked.then(|| view! {
                <span
                    class="flex shrink-0 text-electric-yellow/70"
                    title="Snapshot-locked scene"
                >
                    <Icon icon=LuLock width="11px" height="11px" />
                </span>
            })}
            {move || is_switching().then(|| view! {
                <span class="sr-only">"Switching"</span>
            })}
        </button>
    }
}
