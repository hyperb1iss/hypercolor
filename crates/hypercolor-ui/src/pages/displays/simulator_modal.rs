use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use crate::api;
use crate::display_utils::parse_simulator_dimension;
use crate::icons::*;
use crate::toasts;
use hypercolor_leptos_ext::events::Input;

use super::DisplaysModalBackdrop;

#[component]
pub(super) fn PickerPlaceholder(#[prop(into)] message: String) -> impl IntoView {
    view! {
        <div class="px-3 py-6 text-xs leading-relaxed text-fg-tertiary">{message}</div>
    }
}

#[component]
pub(super) fn CreateSimulatorModal(
    #[prop(into)] on_created: Callback<api::SimulatedDisplaySummary>,
    #[prop(into)] on_close: Callback<()>,
) -> impl IntoView {
    let (name, set_name) = signal("Preview Simulator".to_string());
    let (width, set_width) = signal("480".to_string());
    let (height, set_height) = signal("480".to_string());
    let (circular, set_circular) = signal(true);
    let (submitting, set_submitting) = signal(false);
    let (error, set_error) = signal(None::<String>);

    let submit = {
        move |event: leptos::ev::SubmitEvent| {
            event.prevent_default();
            if submitting.get_untracked() {
                return;
            }

            let width_raw = width.get_untracked();
            let Ok(width) = parse_simulator_dimension(&width_raw, "Width") else {
                set_error.set(parse_simulator_dimension(&width_raw, "Width").err());
                return;
            };
            let height_raw = height.get_untracked();
            let Ok(height) = parse_simulator_dimension(&height_raw, "Height") else {
                set_error.set(parse_simulator_dimension(&height_raw, "Height").err());
                return;
            };

            set_submitting.set(true);
            set_error.set(None);
            let request = api::CreateSimulatedDisplayRequest {
                name: name.get_untracked(),
                width,
                height,
                circular: circular.get_untracked(),
                enabled: true,
            };

            spawn_local(async move {
                match api::create_simulated_display(&request).await {
                    Ok(summary) => {
                        toasts::toast_success("Simulator created");
                        on_created.run(summary);
                    }
                    Err(message) => {
                        set_error.set(Some(message));
                        set_submitting.set(false);
                    }
                }
            });
        }
    };

    let close_button = on_close;

    view! {
        <DisplaysModalBackdrop on_close=on_close>
            <div
                class="w-full max-w-md rounded-xl border border-edge-subtle bg-surface-raised p-4 shadow-2xl"
                on:click=|event| event.stop_propagation()
            >
                <div class="mb-4 flex items-start justify-between gap-3">
                    <div>
                        <h2 class="text-sm font-semibold text-fg-primary">"Create simulator"</h2>
                        <p class="mt-1 text-[11px] leading-relaxed text-fg-tertiary">
                            "Spin up a software LCD that appears in the display list and preview pipeline."
                        </p>
                    </div>
                    <button
                        type="button"
                        class="rounded-sm p-1 text-fg-tertiary transition hover:text-accent-primary"
                        title="Close"
                        on:click=move |_| close_button.run(())
                    >
                        <Icon icon=LuX width="14" height="14" />
                    </button>
                </div>

                <form class="flex flex-col gap-3" on:submit=submit>
                    <label class="flex flex-col gap-1">
                        <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                            "Name"
                        </span>
                        <input
                            type="text"
                            class="rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-primary outline-none transition focus:border-accent-primary"
                            prop:value=move || name.get()
                            on:input=move |event| {
                                set_name.set(Input::from_event(event).value_string().unwrap_or_default())
                            }
                        />
                    </label>

                    <div class="grid grid-cols-2 gap-3">
                        <label class="flex flex-col gap-1">
                            <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                                "Width"
                            </span>
                            <input
                                type="number"
                                min="1"
                                max="4096"
                                class="rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-primary outline-none transition focus:border-accent-primary"
                                prop:value=move || width.get()
                                on:input=move |event| {
                                    set_width.set(Input::from_event(event).value_string().unwrap_or_default())
                                }
                            />
                        </label>
                        <label class="flex flex-col gap-1">
                            <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                                "Height"
                            </span>
                            <input
                                type="number"
                                min="1"
                                max="4096"
                                class="rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-primary outline-none transition focus:border-accent-primary"
                                prop:value=move || height.get()
                                on:input=move |event| {
                                    set_height.set(Input::from_event(event).value_string().unwrap_or_default())
                                }
                            />
                        </label>
                    </div>

                    <div class="flex flex-col gap-1">
                        <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                            "Shape"
                        </span>
                        <div class="grid grid-cols-2 gap-2">
                            <button
                                type="button"
                                class=move || {
                                    if circular.get() {
                                        "flex items-center justify-center gap-2 rounded-md border border-accent-primary bg-accent-primary/10 px-3 py-2 text-sm text-accent-primary transition"
                                    } else {
                                        "flex items-center justify-center gap-2 rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-tertiary transition hover:border-accent-primary/35"
                                    }
                                }
                                on:click=move |_| set_circular.set(true)
                            >
                                <Icon icon=LuCircle width="13" height="13" />
                                "Round"
                            </button>
                            <button
                                type="button"
                                class=move || {
                                    if circular.get() {
                                        "flex items-center justify-center gap-2 rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-tertiary transition hover:border-accent-primary/35"
                                    } else {
                                        "flex items-center justify-center gap-2 rounded-md border border-accent-primary bg-accent-primary/10 px-3 py-2 text-sm text-accent-primary transition"
                                    }
                                }
                                on:click=move |_| set_circular.set(false)
                            >
                                <Icon icon=LuSquare width="13" height="13" />
                                "Square"
                            </button>
                        </div>
                    </div>

                    <Show when=move || error.with(Option::is_some) fallback=|| ()>
                        <div class="rounded-md border border-status-error/35 bg-status-error/10 px-3 py-2 text-xs text-status-error">
                            {move || error.get().unwrap_or_default()}
                        </div>
                    </Show>

                    <div class="mt-1 flex items-center justify-end gap-2">
                        <button
                            type="button"
                            class="rounded-md px-3 py-2 text-xs uppercase tracking-wider text-fg-tertiary transition hover:text-fg-primary"
                            on:click=move |_| on_close.run(())
                        >
                            "Cancel"
                        </button>
                        <button
                            type="submit"
                            class="inline-flex items-center gap-2 rounded-md border border-accent-primary/40 bg-accent-primary/12 px-3 py-2 text-xs font-medium uppercase tracking-wider text-accent-primary transition hover:bg-accent-primary/18 disabled:cursor-not-allowed disabled:opacity-50"
                            disabled=move || submitting.get()
                        >
                            <Icon icon=LuPlus width="12" height="12" />
                            {move || if submitting.get() { "Creating..." } else { "Create simulator" }}
                        </button>
                    </div>
                </form>
            </div>
        </DisplaysModalBackdrop>
    }
}

#[component]
pub(super) fn EditSimulatorModal(
    display: api::DisplaySummary,
    #[prop(into)] on_updated: Callback<api::SimulatedDisplaySummary>,
    #[prop(into)] on_deleted: Callback<String>,
    #[prop(into)] on_close: Callback<()>,
) -> impl IntoView {
    let display_id = display.id.clone();
    let display_name = display.name.clone();
    let (name, set_name) = signal(display.name.clone());
    let (width, set_width) = signal(display.width.to_string());
    let (height, set_height) = signal(display.height.to_string());
    let (circular, set_circular) = signal(display.circular);
    let (submitting, set_submitting) = signal(false);
    let (error, set_error) = signal(None::<String>);

    let submit = {
        let display_id = display_id.clone();
        move |event: leptos::ev::SubmitEvent| {
            event.prevent_default();
            if submitting.get_untracked() {
                return;
            }

            let width_raw = width.get_untracked();
            let Ok(width) = parse_simulator_dimension(&width_raw, "Width") else {
                set_error.set(parse_simulator_dimension(&width_raw, "Width").err());
                return;
            };
            let height_raw = height.get_untracked();
            let Ok(height) = parse_simulator_dimension(&height_raw, "Height") else {
                set_error.set(parse_simulator_dimension(&height_raw, "Height").err());
                return;
            };

            set_submitting.set(true);
            set_error.set(None);
            let request = api::UpdateSimulatedDisplayRequest {
                name: Some(name.get_untracked()),
                width: Some(width),
                height: Some(height),
                circular: Some(circular.get_untracked()),
                enabled: None,
            };

            let display_id = display_id.clone();
            let success_name = name.get_untracked();
            spawn_local(async move {
                match api::patch_simulated_display(&display_id, &request).await {
                    Ok(summary) => {
                        toasts::toast_success(&format!("Updated {success_name}"));
                        on_updated.run(summary);
                    }
                    Err(message) => {
                        set_error.set(Some(message));
                        set_submitting.set(false);
                    }
                }
            });
        }
    };

    let delete_simulator = {
        let display_id = display_id.clone();
        move |_| {
            if submitting.get_untracked() {
                return;
            }

            set_submitting.set(true);
            set_error.set(None);
            let deleted_id = display_id.clone();
            let deleted_name = display_name.clone();
            spawn_local(async move {
                match api::delete_simulated_display(&deleted_id).await {
                    Ok(()) => {
                        toasts::toast_success(&format!("Deleted {deleted_name}"));
                        on_deleted.run(deleted_id.clone());
                    }
                    Err(message) => {
                        set_error.set(Some(message));
                        set_submitting.set(false);
                    }
                }
            });
        }
    };

    let close_button = on_close;

    view! {
        <DisplaysModalBackdrop on_close=on_close>
            <div
                class="w-full max-w-md rounded-xl border border-edge-subtle bg-surface-raised p-4 shadow-2xl"
                on:click=|event| event.stop_propagation()
            >
                <div class="mb-4 flex items-start justify-between gap-3">
                    <div>
                        <h2 class="text-sm font-semibold text-fg-primary">"Simulator settings"</h2>
                        <p class="mt-1 text-[11px] leading-relaxed text-fg-tertiary">
                            "Adjust this virtual LCD or remove it from the daemon."
                        </p>
                    </div>
                    <button
                        type="button"
                        class="rounded-sm p-1 text-fg-tertiary transition hover:text-accent-primary"
                        title="Close"
                        on:click=move |_| close_button.run(())
                    >
                        <Icon icon=LuX width="14" height="14" />
                    </button>
                </div>

                <form class="flex flex-col gap-3" on:submit=submit>
                    <label class="flex flex-col gap-1">
                        <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                            "Name"
                        </span>
                        <input
                            type="text"
                            class="rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-primary outline-none transition focus:border-accent-primary"
                            prop:value=move || name.get()
                            on:input=move |event| {
                                set_name.set(Input::from_event(event).value_string().unwrap_or_default())
                            }
                        />
                    </label>

                    <div class="grid grid-cols-2 gap-3">
                        <label class="flex flex-col gap-1">
                            <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                                "Width"
                            </span>
                            <input
                                type="number"
                                min="1"
                                max="4096"
                                class="rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-primary outline-none transition focus:border-accent-primary"
                                prop:value=move || width.get()
                                on:input=move |event| {
                                    set_width.set(Input::from_event(event).value_string().unwrap_or_default())
                                }
                            />
                        </label>
                        <label class="flex flex-col gap-1">
                            <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                                "Height"
                            </span>
                            <input
                                type="number"
                                min="1"
                                max="4096"
                                class="rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-primary outline-none transition focus:border-accent-primary"
                                prop:value=move || height.get()
                                on:input=move |event| {
                                    set_height.set(Input::from_event(event).value_string().unwrap_or_default())
                                }
                            />
                        </label>
                    </div>

                    <div class="flex flex-col gap-1">
                        <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                            "Shape"
                        </span>
                        <div class="grid grid-cols-2 gap-2">
                            <button
                                type="button"
                                class=move || {
                                    if circular.get() {
                                        "flex items-center justify-center gap-2 rounded-md border border-accent-primary bg-accent-primary/10 px-3 py-2 text-sm text-accent-primary transition"
                                    } else {
                                        "flex items-center justify-center gap-2 rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-tertiary transition hover:border-accent-primary/35"
                                    }
                                }
                                on:click=move |_| set_circular.set(true)
                            >
                                <Icon icon=LuCircle width="13" height="13" />
                                "Round"
                            </button>
                            <button
                                type="button"
                                class=move || {
                                    if circular.get() {
                                        "flex items-center justify-center gap-2 rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-tertiary transition hover:border-accent-primary/35"
                                    } else {
                                        "flex items-center justify-center gap-2 rounded-md border border-accent-primary bg-accent-primary/10 px-3 py-2 text-sm text-accent-primary transition"
                                    }
                                }
                                on:click=move |_| set_circular.set(false)
                            >
                                <Icon icon=LuSquare width="13" height="13" />
                                "Square"
                            </button>
                        </div>
                    </div>

                    <Show when=move || error.with(Option::is_some) fallback=|| ()>
                        <div class="rounded-md border border-status-error/35 bg-status-error/10 px-3 py-2 text-xs text-status-error">
                            {move || error.get().unwrap_or_default()}
                        </div>
                    </Show>

                    <div class="mt-1 flex items-center justify-between gap-2">
                        <button
                            type="button"
                            class="inline-flex items-center gap-2 rounded-md border border-status-error/35 bg-status-error/10 px-3 py-2 text-xs font-medium uppercase tracking-wider text-status-error transition hover:bg-status-error/15 disabled:cursor-not-allowed disabled:opacity-50"
                            disabled=move || submitting.get()
                            on:click=delete_simulator
                        >
                            <Icon icon=LuTrash2 width="12" height="12" />
                            "Delete simulator"
                        </button>
                        <div class="flex items-center gap-2">
                            <button
                                type="button"
                                class="rounded-md px-3 py-2 text-xs uppercase tracking-wider text-fg-tertiary transition hover:text-fg-primary"
                                on:click=move |_| on_close.run(())
                            >
                                "Cancel"
                            </button>
                            <button
                                type="submit"
                                class="inline-flex items-center gap-2 rounded-md border border-accent-primary/40 bg-accent-primary/12 px-3 py-2 text-xs font-medium uppercase tracking-wider text-accent-primary transition hover:bg-accent-primary/18 disabled:cursor-not-allowed disabled:opacity-50"
                                disabled=move || submitting.get()
                            >
                                <Icon icon=LuSave width="12" height="12" />
                                {move || if submitting.get() { "Saving..." } else { "Save simulator" }}
                            </button>
                        </div>
                    </div>
                </form>
            </div>
        </DisplaysModalBackdrop>
    }
}
