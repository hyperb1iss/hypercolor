use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_leptos_ext::events::Input;

use crate::icons::*;

/// Action button group — extracted to keep tuple sizes manageable.
#[component]
pub(super) fn PresetActionButtons(
    has_selection: Memo<bool>,
    on_save: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_new: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_edit: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_delete: impl Fn(leptos::ev::MouseEvent) + 'static,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-0.5 shrink-0">
            // Save (overwrite current preset)
            <button
                class="p-1.5 rounded-lg text-fg-tertiary/40 toolbar-action
                       hover:text-success-green hover:bg-success-green/10"
                title="Save controls to preset"
                aria-label="Save controls to preset"
                disabled=move || !has_selection.get()
                on:click=on_save
            >
                <Icon icon=LuSave width="14px" height="14px" />
            </button>

            // New preset
            <button
                class="p-1.5 rounded-lg text-fg-tertiary/40 toolbar-action
                       hover:text-neon-cyan hover:bg-neon-cyan/10"
                title="Create new preset"
                aria-label="Create new preset"
                on:click=on_new
            >
                <Icon icon=LuPlus width="14px" height="14px" />
            </button>

            // Edit name
            <button
                class="p-1.5 rounded-lg text-fg-tertiary/40 toolbar-action
                       hover:text-electric-purple hover:bg-electric-purple/10"
                title="Rename preset"
                aria-label="Rename preset"
                disabled=move || !has_selection.get()
                on:click=on_edit
            >
                <Icon icon=LuSquarePen width="14px" height="14px" />
            </button>

            // Delete
            <button
                class="p-1.5 rounded-lg text-fg-tertiary/40 toolbar-action
                       hover:text-error-red hover:bg-error-red/10"
                title="Delete preset"
                aria-label="Delete preset"
                disabled=move || !has_selection.get()
                on:click=on_delete
            >
                <Icon icon=LuTrash2 width="14px" height="14px" />
            </button>
        </div>
    }
}

/// Inline text input for creating or renaming a preset.
#[component]
pub(super) fn InlineNameInput(
    placeholder: &'static str,
    #[prop(into)] initial: String,
    on_submit: Callback<String>,
    on_cancel: Callback<()>,
) -> impl IntoView {
    let (value, set_value) = signal(initial);

    view! {
        <div class="flex items-center gap-2">
            <input
                type="text"
                placeholder=placeholder
                class="flex-1 bg-surface-sunken/60 border border-accent-muted/60 rounded-lg px-2.5 py-1.5
                       text-xs text-fg-primary placeholder-fg-tertiary/40
                       focus:outline-none focus:border-accent glow-ring
                       transition-all duration-200"
                prop:value=move || value.get()
                on:input=move |ev| {
                    let event = Input::from_event(ev);
                    if let Some(value) = event.value_string() {
                        set_value.set(value);
                    }
                }
                on:keydown=move |ev| {
                    if ev.key() == "Enter" {
                        let name = value.get().trim().to_string();
                        if !name.is_empty() {
                            on_submit.run(name);
                        }
                    } else if ev.key() == "Escape" {
                        on_cancel.run(());
                    }
                }
            />
            <InlineNameButtons
                value=value
                on_submit=on_submit
                on_cancel=on_cancel
            />
        </div>
    }
}

/// Confirm/Cancel buttons for inline name input.
#[component]
fn InlineNameButtons(
    value: ReadSignal<String>,
    on_submit: Callback<String>,
    on_cancel: Callback<()>,
) -> impl IntoView {
    view! {
        <button
            class="p-1.5 rounded-lg text-fg-tertiary/40 toolbar-action
                   hover:text-success-green hover:bg-success-green/10"
            title="Confirm"
            disabled=move || value.get().trim().is_empty()
            on:click=move |_| {
                let name = value.get().trim().to_string();
                if !name.is_empty() {
                    on_submit.run(name);
                }
            }
        >
            <Icon icon=LuCheck width="14px" height="14px" />
        </button>
        <button
            class="p-1.5 rounded-lg text-fg-tertiary/40 toolbar-action
                   hover:text-error-red hover:bg-error-red/10"
            title="Cancel"
            on:click=move |_| on_cancel.run(())
        >
            <Icon icon=LuX width="14px" height="14px" />
        </button>
    }
}
