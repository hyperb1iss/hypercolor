//! Preset toolbar — compact single-line preset selector with save/create/edit/delete.

use leptos::prelude::*;
use leptos_icons::Icon;
use std::collections::HashMap;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use hypercolor_types::effect::{ControlValue, PresetTemplate};

use super::preset_matching::{
    bundled_preset_matches_controls, bundled_preset_to_json, controls_to_json,
    user_preset_matches_controls,
};
use crate::api;
use crate::icons::*;
use crate::toasts;

/// Compact preset toolbar for the effect detail sidebar.
///
/// Single line: `[Preset dropdown] [Save] [+] [···]`
#[component]
pub fn PresetToolbar(
    /// The active effect's ID.
    #[prop(into)]
    effect_id: Signal<Option<String>>,
    /// Current live control values — snapshotted when saving.
    #[prop(into)]
    control_values: Signal<HashMap<String, ControlValue>>,
    /// Category accent color as "r, g, b" string.
    #[prop(into)]
    #[allow(unused)]
    accent_rgb: Signal<String>,
    /// Callback fired after a preset is applied (so parent can refresh controls).
    #[prop(into)]
    on_preset_applied: Callback<()>,
    /// The active preset ID from the engine (restored on effect switch).
    #[prop(into, optional)]
    active_preset_id_signal: Option<Signal<Option<String>>>,
) -> impl IntoView {
    let (presets, set_presets) = signal(Vec::<api::PresetSummary>::new());
    let (bundled_presets, set_bundled_presets) = signal(Vec::<PresetTemplate>::new());
    let (selected_id, set_selected_id) = signal(Option::<String>::None);
    let (mode, set_mode) = signal(ToolbarMode::Idle);

    // Fetch user presets + bundled presets whenever effect_id *actually* changes.
    //
    // Leptos signals always notify on `set()` (no PartialEq guard), so the
    // derived `effect_id` signal can re-fire even when the ID is unchanged
    // (e.g., after `refresh_active_effect`). We compare against the previous
    // value and skip the fetch+clear when nothing changed — this prevents
    // option recreation from resetting the <select> element.
    Effect::new(move |prev_eid: Option<Option<String>>| {
        let eid = effect_id.get();
        if prev_eid.as_ref() == Some(&eid) {
            return eid; // Same effect — skip everything
        }

        set_selected_id.set(None);
        let fetch_eid = eid.clone();
        leptos::task::spawn_local(async move {
            match api::fetch_presets().await {
                Ok(all) => set_presets.set(all),
                Err(_) => set_presets.set(Vec::new()),
            }
            // Fetch bundled presets from effect detail
            if let Some(ref id) = fetch_eid {
                match api::fetch_bundled_presets(id).await {
                    Ok(bp) => set_bundled_presets.set(bp),
                    Err(_) => set_bundled_presets.set(Vec::new()),
                }
            } else {
                set_bundled_presets.set(Vec::new());
            }
        });
        eid
    });

    // Keep the UI selection aligned with whichever preset the current control
    // values actually match. Built-in presets do not have engine-backed IDs,
    // so restoring from control values is the only reliable source of truth.
    Effect::new(move |_| {
        let eid = effect_id.get();
        let active_preset_id = active_preset_id_signal
            .map(|signal| signal.get())
            .unwrap_or_default();
        let current_values = control_values.get();
        let current_presets = presets.get();
        let current_bundled = bundled_presets.get();

        let next_selected = eid.as_ref().and_then(|active_effect_id| {
            active_preset_id
                .filter(|preset_id| {
                    current_presets.iter().any(|preset| {
                        preset.effect_id == *active_effect_id && preset.id == *preset_id
                    })
                })
                .or_else(|| {
                    current_presets
                        .iter()
                        .find(|preset| {
                            preset.effect_id == *active_effect_id
                                && user_preset_matches_controls(&current_values, &preset.controls)
                        })
                        .map(|preset| preset.id.clone())
                })
                .or_else(|| {
                    current_bundled
                        .iter()
                        .enumerate()
                        .find(|(_, preset)| {
                            bundled_preset_matches_controls(&current_values, &preset.controls)
                        })
                        .map(|(index, _)| format!("bundled:{index}"))
                })
        });

        if selected_id.get_untracked() != next_selected {
            set_selected_id.set(next_selected);
        }
    });

    // Filter presets to the active effect
    let effect_presets = Memo::new(move |_| {
        let eid = effect_id.get().unwrap_or_default();
        presets
            .get()
            .into_iter()
            .filter(|p| p.effect_id == eid)
            .collect::<Vec<_>>()
    });

    let selected_preset = Memo::new(move |_| {
        let sid = selected_id.get()?;
        effect_presets.get().into_iter().find(|p| p.id == sid)
    });

    let has_editable_selection = Memo::new(move |_| selected_preset.get().is_some());

    // Refresh helper
    let refresh_presets = move || {
        leptos::task::spawn_local(async move {
            if let Ok(all) = api::fetch_presets().await {
                set_presets.set(all);
            }
        });
    };

    // Select preset by value string (replaces the old on_select that took web_sys::Event)
    let on_select_value = move |val: String| {
        if val.is_empty() {
            // "No preset" selected — reset controls to defaults
            set_selected_id.set(None);
            let on_applied = on_preset_applied;
            leptos::task::spawn_local(async move {
                if api::reset_controls().await.is_ok() {
                    on_applied.run(());
                }
            });
            return;
        }

        // Handle bundled preset selection (value = "bundled:<index>")
        if let Some(idx_str) = val.strip_prefix("bundled:") {
            if let Ok(idx) = idx_str.parse::<usize>() {
                let bp = bundled_presets.get();
                if let Some(template) = bp.get(idx) {
                    let controls_json = bundled_preset_to_json(&template.controls);
                    set_selected_id.set(Some(val));
                    set_mode.set(ToolbarMode::Idle);
                    let on_applied = on_preset_applied;
                    leptos::task::spawn_local(async move {
                        if api::update_controls(&controls_json).await.is_ok() {
                            on_applied.run(());
                        }
                    });
                }
            }
            return;
        }

        set_selected_id.set(Some(val.clone()));
        set_mode.set(ToolbarMode::Idle);
        let on_applied = on_preset_applied;
        leptos::task::spawn_local(async move {
            if api::apply_preset(&val).await.is_ok() {
                on_applied.run(());
            }
        });
    };

    // Save over current preset
    let on_save = move |_: leptos::ev::MouseEvent| {
        let Some(preset) = selected_preset.get() else {
            return;
        };
        let eid = effect_id.get().unwrap_or_default();
        let values = control_values.get();
        let controls_json = controls_to_json(&values);
        let name = preset.name.clone();
        let pid = preset.id.clone();
        let refresh = refresh_presets;
        leptos::task::spawn_local(async move {
            let req = api::CreatePresetRequest {
                name,
                description: None,
                effect: eid,
                controls: serde_json::Value::Object(controls_json),
                tags: None,
            };
            if api::update_preset(&pid, &req).await.is_ok() {
                toasts::toast_success("Preset saved");
                refresh();
            }
        });
    };

    // Create new preset
    let on_create = move |name: String| {
        let eid = effect_id.get().unwrap_or_default();
        let values = control_values.get();
        let controls_json = controls_to_json(&values);
        let refresh = refresh_presets;
        set_mode.set(ToolbarMode::Idle);
        leptos::task::spawn_local(async move {
            let req = api::CreatePresetRequest {
                name,
                description: None,
                effect: eid,
                controls: serde_json::Value::Object(controls_json),
                tags: None,
            };
            if let Ok(created) = api::create_preset(&req).await {
                set_selected_id.set(Some(created.id));
                toasts::toast_success("Preset created");
                refresh();
            }
        });
    };

    // Rename preset
    let on_rename = move |new_name: String| {
        let Some(preset) = selected_preset.get() else {
            return;
        };
        let eid = effect_id.get().unwrap_or_default();
        let pid = preset.id.clone();
        let refresh = refresh_presets;
        set_mode.set(ToolbarMode::Idle);
        leptos::task::spawn_local(async move {
            let req = api::CreatePresetRequest {
                name: new_name,
                description: None,
                effect: eid,
                controls: serde_json::Value::Object(
                    preset
                        .controls
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                ),
                tags: None,
            };
            if api::update_preset(&pid, &req).await.is_ok() {
                toasts::toast_success("Preset renamed");
                refresh();
            }
        });
    };

    // Delete preset
    let on_delete = move |_: leptos::ev::MouseEvent| {
        let Some(preset) = selected_preset.get() else {
            return;
        };
        let pid = preset.id.clone();
        let refresh = refresh_presets;
        set_selected_id.set(None);
        set_mode.set(ToolbarMode::Idle);
        leptos::task::spawn_local(async move {
            if api::delete_preset(&pid).await.is_ok() {
                toasts::toast_info("Preset deleted");
                refresh();
            }
        });
    };

    view! {
        <div class="py-0.5">
            {move || {
                match mode.get() {
                    ToolbarMode::Idle => {
                        let on_save = on_save;
                        let on_delete = on_delete;
                        view! {
                            <div class="animate-swap-in">
                                <PresetSelectorRow
                                    effect_presets=effect_presets
                                    bundled_presets=bundled_presets
                                    selected_id=selected_id
                                    has_editable_selection=has_editable_selection
                                    on_select=Callback::new(on_select_value)
                                    on_save=on_save
                                    on_new=move |_| set_mode.set(ToolbarMode::Creating)
                                    on_edit=move |_| set_mode.set(ToolbarMode::Renaming)
                                    on_delete=on_delete
                                />
                            </div>
                        }.into_any()
                    }
                    ToolbarMode::Creating => {
                        let on_create = on_create;
                        view! {
                            <div class="animate-swap-in">
                                <InlineNameInput
                                    placeholder="New preset name..."
                                    initial=""
                                    on_submit=Callback::new(move |name: String| on_create(name))
                                    on_cancel=Callback::new(move |()| set_mode.set(ToolbarMode::Idle))
                                />
                            </div>
                        }.into_any()
                    }
                    ToolbarMode::Renaming => {
                        let current_name = selected_preset
                            .get()
                            .map(|p| p.name.clone())
                            .unwrap_or_default();
                        let on_rename = on_rename;
                        view! {
                            <div class="animate-swap-in">
                                <InlineNameInput
                                    placeholder="Rename preset..."
                                    initial=current_name
                                    on_submit=Callback::new(move |name: String| on_rename(name))
                                    on_cancel=Callback::new(move |()| set_mode.set(ToolbarMode::Idle))
                                />
                            </div>
                        }.into_any()
                    }
                }
            }}
        </div>
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolbarMode {
    Idle,
    Creating,
    Renaming,
}

/// The main selector row: custom dropdown + action buttons.
#[component]
fn PresetSelectorRow(
    effect_presets: Memo<Vec<api::PresetSummary>>,
    bundled_presets: ReadSignal<Vec<PresetTemplate>>,
    selected_id: ReadSignal<Option<String>>,
    has_editable_selection: Memo<bool>,
    on_select: Callback<String>,
    on_save: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_new: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_edit: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_delete: impl Fn(leptos::ev::MouseEvent) + 'static,
) -> impl IntoView {
    let (is_open, set_is_open) = signal(false);

    // Build the display label for the currently selected item
    let selected_label = Memo::new(move |_| {
        let sid = selected_id.get();
        let Some(ref sid) = sid else {
            return "Default".to_string();
        };

        // Check bundled presets
        if let Some(idx_str) = sid.strip_prefix("bundled:")
            && let Ok(idx) = idx_str.parse::<usize>() {
                let bp = bundled_presets.get();
                if let Some(template) = bp.get(idx) {
                    return format!("\u{2726} {}", template.name);
                }
            }

        // Check user presets
        effect_presets
            .get()
            .iter()
            .find(|p| p.id == *sid)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Default".to_string())
    });

    // Click-outside handler — close dropdown when clicking outside
    install_dropdown_outside_handler(set_is_open);

    // Close on Escape
    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Escape" && is_open.get_untracked() {
            set_is_open.set(false);
            ev.prevent_default();
        }
    };

    view! {
        <div class="flex items-center gap-2" on:keydown=on_keydown>
            // Custom dropdown
            <div class="relative flex-1 min-w-0 preset-dropdown">
                // Trigger button
                <button
                    type="button"
                    class="w-full flex items-center gap-1.5 bg-surface-sunken/60 border px-2.5 py-1.5
                           text-xs cursor-pointer select-silk-trigger"
                    class=("rounded-t-lg", move || is_open.get())
                    class=("rounded-lg", move || !is_open.get())
                    class=("border-accent-muted", move || is_open.get())
                    class=("border-edge-subtle", move || !is_open.get())
                    on:click=move |_| set_is_open.update(|v| *v = !*v)
                >
                    <span class="flex-1 min-w-0 text-left truncate text-fg-primary">
                        {move || selected_label.get()}
                    </span>
                    <svg
                        class="w-3 h-3 shrink-0 transition-transform duration-200"
                        class=("rotate-180", move || is_open.get())
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                    >
                        <path d="m6 9 6 6 6-6" />
                    </svg>
                </button>

                // Dropdown popover
                <Show when=move || is_open.get()>
                    <div
                        class="absolute left-0 right-0 top-full
                               rounded-b-xl overflow-hidden
                               bg-surface-overlay/98 backdrop-blur-xl
                               border border-t-0 border-edge-subtle
                               dropdown-glow animate-slide-down
                               max-h-[320px] overflow-y-auto scrollbar-dropdown"
                        style="z-index: 9999; margin-top: -1px"
                        on:mousedown=|ev: leptos::ev::MouseEvent| ev.stop_propagation()
                    >
                        // Default preset option — resets controls to effect defaults
                        <DropdownItem
                            value="".to_string()
                            label="Default".to_string()
                            is_selected=Signal::derive(move || selected_id.get().is_none())
                            on_click=Callback::new(move |val: String| {
                                on_select.run(val);
                                set_is_open.set(false);
                            })
                        />

                        // Bundled presets group
                        {move || {
                            let bp = bundled_presets.get();
                            let has_user = !effect_presets.get().is_empty();
                            if bp.is_empty() {
                                return {
                                    let _: () = view! { <></> };
                                    ().into_any()
                                };
                            }
                            view! {
                                <>
                                    {(has_user).then(|| view! {
                                        <div class="px-2.5 pt-2 pb-1">
                                            <span class="text-[9px] font-mono uppercase tracking-[0.15em] text-fg-tertiary/50">
                                                "Built-in"
                                            </span>
                                        </div>
                                    })}
                                    {bp.into_iter().enumerate().map(|(idx, p)| {
                                        let val = format!("bundled:{idx}");
                                        let label = format!("\u{2726} {}", p.name);
                                        let option_value = val.clone();
                                        view! {
                                            <DropdownItem
                                                value=val
                                                label=label
                                                is_selected=Signal::derive(move || selected_id.get().as_deref() == Some(option_value.as_str()))
                                                on_click=Callback::new(move |val: String| {
                                                    on_select.run(val);
                                                    set_is_open.set(false);
                                                })
                                            />
                                        }
                                    }).collect_view()}
                                </>
                            }.into_any()
                        }}

                        // User presets group
                        {move || {
                            let user = effect_presets.get();
                            let has_bundled = !bundled_presets.get().is_empty();
                            if user.is_empty() {
                                return {
                                    let _: () = view! { <></> };
                                    ().into_any()
                                };
                            }
                            view! {
                                <>
                                    {(has_bundled).then(|| view! {
                                        <div class="px-2.5 pt-2 pb-1">
                                            <span class="text-[9px] font-mono uppercase tracking-[0.15em] text-fg-tertiary/50">
                                                "My Presets"
                                            </span>
                                        </div>
                                    })}
                                    {user.into_iter().map(|p| {
                                        let id = p.id.clone();
                                        let option_value = id.clone();
                                        view! {
                                            <DropdownItem
                                                value=id
                                                label=p.name
                                                is_selected=Signal::derive(move || selected_id.get().as_deref() == Some(option_value.as_str()))
                                                on_click=Callback::new(move |val: String| {
                                                    on_select.run(val);
                                                    set_is_open.set(false);
                                                })
                                            />
                                        }
                                    }).collect_view()}
                                </>
                            }.into_any()
                        }}
                    </div>
                </Show>
            </div>

            // Action buttons
            <PresetActionButtons
                has_selection=has_editable_selection
                on_save=on_save
                on_new=on_new
                on_edit=on_edit
                on_delete=on_delete
            />
        </div>
    }
}

/// A single item in the custom dropdown.
#[component]
fn DropdownItem(
    #[prop(into)] value: String,
    #[prop(into)] label: String,
    #[prop(into)] is_selected: Signal<bool>,
    on_click: Callback<String>,
) -> impl IntoView {
    let val = value.clone();
    view! {
        <button
            type="button"
            class="dropdown-option w-full text-left px-3 py-[7px] text-xs cursor-pointer
                   flex items-center gap-2"
            class=("dropdown-option-active", move || is_selected.get())
            class=("text-fg-tertiary", move || !is_selected.get())
            on:click=move |_| on_click.run(val.clone())
        >
            <span
                class="w-1 h-1 rounded-full shrink-0 transition-all duration-200"
                class=("bg-accent-muted", move || is_selected.get())
                class=("scale-100 opacity-100", move || is_selected.get())
                class=("scale-0 opacity-0", move || !is_selected.get())
            />
            <span class="truncate">{label}</span>
        </button>
    }
}

/// Install a one-time document-level mousedown listener that closes the
/// dropdown when clicking outside `.preset-dropdown`.
fn install_dropdown_outside_handler(set_open: WriteSignal<bool>) {
    let handler = Closure::<dyn Fn(web_sys::Event)>::new(move |ev: web_sys::Event| {
        let inside = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .map(|el| el.closest(".preset-dropdown").ok().flatten().is_some())
            .unwrap_or(false);

        if !inside {
            set_open.set(false);
        }
    });

    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        let _ = doc.add_event_listener_with_callback("mousedown", handler.as_ref().unchecked_ref());
    }
    handler.forget();
}

/// Action button group — extracted to keep tuple sizes manageable.
#[component]
fn PresetActionButtons(
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
fn InlineNameInput(
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
                    let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                    if let Some(el) = target {
                        set_value.set(el.value());
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
