//! The Studio scene selector — the headline control of the workspace.
//!
//! Lists every scene, switches the active one, and creates / renames /
//! deletes scenes. It replaces the layouts-library "Room" picker (plan 55
//! Wave B2): a saved arrangement is not a first-class object, it is part
//! of a scene, so the thing the user picks is a **scene**.
//!
//! Mounted in the Studio `PageHeader` toolbar. Scene mutations refetch the
//! scene list locally and call [`StudioContext::refresh_scene`] so the
//! tree and Stage pick up the new active scene.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;
use web_sys::KeyboardEvent;

use crate::api;
use crate::components::control_panel::ControlDropdownDismissHandlers;
use crate::components::silk_select::SilkSelect;
use crate::icons::*;
use crate::toasts;
use hypercolor_leptos_ext::events::Input;

use super::StudioContext;

/// The scene picker plus its new / rename / delete controls.
#[component]
pub fn SceneSelector() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let active_scene = studio.active_scene;
    let refresh_scene = studio.refresh_scene;

    // The scene list drives the picker. `list_scenes` omits the daemon's
    // ephemeral default scene; the active scene is folded into `options`
    // below so the picker can always name what is on screen.
    let (list_tick, set_list_tick) = signal(0_u64);
    let scenes = LocalResource::new(move || {
        let _ = list_tick.get();
        async move { api::list_scenes().await }
    });
    let refetch_scenes = move || set_list_tick.update(|tick| *tick = tick.wrapping_add(1));

    let (renaming, set_renaming) = signal(false);
    let (rename_value, set_rename_value) = signal(String::new());
    let (creating, set_creating) = signal(false);
    let (new_name, set_new_name) = signal(String::new());
    let (menu_open, set_menu_open) = signal(false);

    let active_id = Signal::derive(move || {
        active_scene.with(|scene| scene.as_ref().map(|scene| scene.id.clone()))
    });
    let active_name = Signal::derive(move || {
        active_scene.with(|scene| scene.as_ref().map(|scene| scene.name.clone()))
    });

    // `(id, name)` options for the picker, in list order. The active scene
    // is prepended when the list omits it — a fresh daemon's ephemeral
    // default scene is never listed but is still the one on screen.
    let options = Signal::derive(move || {
        let listed = scenes.get().and_then(Result::ok).unwrap_or_default();
        let mut opts: Vec<(String, String)> = listed
            .iter()
            .map(|scene| (scene.id.clone(), scene.name.clone()))
            .collect();
        active_scene.with(|scene| {
            if let Some(scene) = scene
                && !opts.iter().any(|(id, _)| id == &scene.id)
            {
                opts.insert(0, (scene.id.clone(), scene.name.clone()));
            }
        });
        opts
    });
    let value = Signal::derive(move || active_id.get().unwrap_or_default());

    // Rename and delete act on the active scene, so they are offered only
    // when it is a real, listed scene — never the ephemeral default.
    let active_is_listed = Signal::derive(move || {
        let Some(id) = active_id.get() else {
            return false;
        };
        scenes
            .get()
            .and_then(Result::ok)
            .is_some_and(|list| list.iter().any(|scene| scene.id == id))
    });
    // The active scene's description, echoed back verbatim on rename: the
    // daemon's PUT replaces the field wholesale, so omitting it clears it.
    let active_description = Signal::derive(move || {
        let id = active_id.get()?;
        scenes
            .get()
            .and_then(Result::ok)?
            .into_iter()
            .find(|scene| scene.id == id)
            .and_then(|scene| scene.description)
    });

    let activate = Callback::new(move |id: String| {
        if active_id.get_untracked().as_deref() == Some(id.as_str()) {
            return;
        }
        spawn_local(async move {
            match api::activate_scene(&id).await {
                Ok(()) => {
                    refresh_scene.run(());
                    refetch_scenes();
                }
                Err(error) => toasts::toast_error(&format!("Couldn't switch scene: {error}")),
            }
        });
    });

    let commit_create = Callback::new(move |()| {
        let name = new_name.get_untracked().trim().to_owned();
        set_creating.set(false);
        set_new_name.set(String::new());
        if name.is_empty() {
            return;
        }
        spawn_local(async move {
            match api::create_scene(&name).await {
                Ok(summary) => {
                    refetch_scenes();
                    match api::activate_scene(&summary.id).await {
                        Ok(()) => {
                            refresh_scene.run(());
                            toasts::toast_success("Scene created");
                        }
                        Err(error) => {
                            toasts::toast_error(&format!("Created, but couldn't switch: {error}"));
                        }
                    }
                }
                Err(error) => toasts::toast_error(&format!("Couldn't create scene: {error}")),
            }
        });
    });

    let commit_rename = Callback::new(move |()| {
        let name = rename_value.get_untracked().trim().to_owned();
        set_renaming.set(false);
        let Some(id) = active_id.get_untracked() else {
            return;
        };
        if name.is_empty() || active_name.get_untracked().as_deref() == Some(name.as_str()) {
            return;
        }
        let description = active_description.get_untracked();
        spawn_local(async move {
            match api::rename_scene(&id, &name, description.as_deref()).await {
                Ok(()) => {
                    refresh_scene.run(());
                    refetch_scenes();
                    toasts::toast_success("Scene renamed");
                }
                Err(error) => toasts::toast_error(&format!("Rename failed: {error}")),
            }
        });
    });

    let delete = Callback::new(move |()| {
        set_menu_open.set(false);
        let Some(id) = active_id.get_untracked() else {
            return;
        };
        let name = active_name.get_untracked().unwrap_or_default();
        spawn_local(async move {
            match api::delete_scene(&id).await {
                Ok(()) => {
                    refresh_scene.run(());
                    refetch_scenes();
                    toasts::toast_info(&format!("Deleted {name}"));
                }
                Err(error) => toasts::toast_error(&format!("Delete failed: {error}")),
            }
        });
    });

    let start_rename = move || {
        set_rename_value.set(active_name.get_untracked().unwrap_or_default());
        set_renaming.set(true);
    };

    view! {
        <div class="flex items-center gap-2">
            <span class="text-[11px] font-semibold uppercase tracking-[0.14em] text-fg-tertiary/65">
                "Scene"
            </span>

            // Picker, or the inline rename input.
            {move || {
                if renaming.get() {
                    view! {
                        <input
                            type="text"
                            class="w-52 rounded-lg border border-edge-subtle bg-surface-sunken px-3 py-1.5 text-sm text-fg-primary placeholder-fg-tertiary transition-all focus:border-accent-muted focus:outline-none glow-ring"
                            prop:value=move || rename_value.get()
                            autofocus=true
                            on:input=move |ev| {
                                if let Some(value) = Input::from_event(ev).value_string() {
                                    set_rename_value.set(value);
                                }
                            }
                            on:blur=move |_| commit_rename.run(())
                            on:keydown=move |ev: KeyboardEvent| {
                                if ev.key() == "Enter" {
                                    commit_rename.run(());
                                } else if ev.key() == "Escape" {
                                    set_renaming.set(false);
                                }
                            }
                        />
                    }
                        .into_any()
                } else {
                    view! {
                        <div class="min-w-[200px]">
                            <SilkSelect
                                value=value
                                options=options
                                on_change=activate
                                placeholder="No scene"
                                class="border border-edge-subtle bg-surface-sunken px-3 py-1.5 text-sm text-fg-primary glow-ring"
                            />
                        </div>
                    }
                        .into_any()
                }
            }}

            // New scene, or the inline create input.
            {move || {
                if creating.get() {
                    view! {
                        <input
                            type="text"
                            placeholder="Scene name"
                            class="w-44 rounded-lg border border-edge-subtle bg-surface-sunken px-3 py-1.5 text-sm text-fg-primary placeholder-fg-tertiary transition-all focus:border-accent-muted focus:outline-none glow-ring"
                            prop:value=move || new_name.get()
                            autofocus=true
                            on:input=move |ev| {
                                if let Some(value) = Input::from_event(ev).value_string() {
                                    set_new_name.set(value);
                                }
                            }
                            on:blur=move |_| commit_create.run(())
                            on:keydown=move |ev: KeyboardEvent| {
                                if ev.key() == "Enter" {
                                    commit_create.run(());
                                } else if ev.key() == "Escape" {
                                    set_creating.set(false);
                                }
                            }
                        />
                    }
                        .into_any()
                } else {
                    view! {
                        <button
                            type="button"
                            class="flex items-center gap-1 whitespace-nowrap rounded-lg border px-3 py-1.5 text-xs font-medium transition-all btn-press"
                            style="background: rgba(225, 53, 255, 0.08); border-color: rgba(225, 53, 255, 0.2); color: rgb(225, 53, 255)"
                            title="New scene"
                            on:click=move |_| {
                                set_new_name.set(String::new());
                                set_creating.set(true);
                            }
                        >
                            <Icon icon=LuPlus width="12px" height="12px" />
                            "New"
                        </button>
                    }
                        .into_any()
                }
            }}

            // Overflow menu — rename / delete the active scene.
            <Show when=move || active_is_listed.get()>
                <div class="relative scene-action-menu">
                    <button
                        type="button"
                        class="flex h-8 w-8 items-center justify-center rounded-md text-fg-tertiary transition-all hover:bg-surface-hover/40 hover:text-fg-primary btn-press"
                        title="Scene actions"
                        on:click=move |_| set_menu_open.update(|open| *open = !*open)
                    >
                        <Icon icon=LuEllipsis width="15px" height="15px" />
                    </button>
                    <Show when=move || menu_open.get()>
                        <ControlDropdownDismissHandlers
                            class_name="scene-action-menu".to_string()
                            is_open=menu_open
                            set_open=set_menu_open
                        />
                        <div
                            class="absolute left-0 top-full z-[100] mt-1 w-44 overflow-hidden rounded-lg border border-edge-subtle bg-surface-overlay/98 backdrop-blur-xl dropdown-glow animate-enter-down"
                            on:keydown=move |ev: KeyboardEvent| {
                                if ev.key() == "Escape" {
                                    set_menu_open.set(false);
                                }
                            }
                        >
                            <button
                                type="button"
                                class="dropdown-option flex w-full items-center gap-2 px-3 py-2 text-left text-xs text-fg-secondary hover:text-fg-primary"
                                on:click=move |_| {
                                    set_menu_open.set(false);
                                    start_rename();
                                }
                            >
                                <Icon
                                    icon=LuPencil
                                    width="12px"
                                    height="12px"
                                    style="color: rgba(139, 133, 160, 0.7); flex-shrink: 0"
                                />
                                <span>"Rename"</span>
                            </button>
                            <div class="mx-2 my-1 h-px bg-edge-subtle/40" />
                            <button
                                type="button"
                                class="dropdown-option flex w-full items-center gap-2 px-3 py-2 text-left text-xs text-status-error/70 hover:text-status-error"
                                on:click=move |_| delete.run(())
                            >
                                <Icon
                                    icon=LuTrash2
                                    width="12px"
                                    height="12px"
                                    style="color: rgba(255, 99, 99, 0.7); flex-shrink: 0"
                                />
                                <span>"Delete"</span>
                            </button>
                        </div>
                    </Show>
                </div>
            </Show>
        </div>
    }
}
