//! Preset toolbar — compact single-line preset selector with save/create/edit/delete.

use leptos::prelude::*;
use leptos_icons::Icon;
use std::collections::HashMap;
use wasm_bindgen::JsCast;

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

    // Select preset from dropdown → apply it (or reset to defaults for "No preset")
    let on_select = move |ev: web_sys::Event| {
        let target = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
        let Some(el) = target else { return };
        let val = el.value();
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
        <div>
            {move || {
                match mode.get() {
                    ToolbarMode::Idle => {
                        let on_save = on_save;
                        let on_delete = on_delete;
                        view! {
                            <PresetSelectorRow
                                effect_presets=effect_presets
                                bundled_presets=bundled_presets
                                selected_id=selected_id
                                has_editable_selection=has_editable_selection
                                on_select=on_select
                                on_save=on_save
                                on_new=move |_| set_mode.set(ToolbarMode::Creating)
                                on_edit=move |_| set_mode.set(ToolbarMode::Renaming)
                                on_delete=on_delete
                            />
                        }.into_any()
                    }
                    ToolbarMode::Creating => {
                        let on_create = on_create;
                        view! {
                            <InlineNameInput
                                placeholder="New preset name..."
                                initial=""
                                on_submit=Callback::new(move |name: String| on_create(name))
                                on_cancel=Callback::new(move |()| set_mode.set(ToolbarMode::Idle))
                            />
                        }.into_any()
                    }
                    ToolbarMode::Renaming => {
                        let current_name = selected_preset
                            .get()
                            .map(|p| p.name.clone())
                            .unwrap_or_default();
                        let on_rename = on_rename;
                        view! {
                            <InlineNameInput
                                placeholder="Rename preset..."
                                initial=current_name
                                on_submit=Callback::new(move |name: String| on_rename(name))
                                on_cancel=Callback::new(move |()| set_mode.set(ToolbarMode::Idle))
                            />
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

/// The main selector row: dropdown + action buttons.
#[component]
fn PresetSelectorRow(
    effect_presets: Memo<Vec<api::PresetSummary>>,
    bundled_presets: ReadSignal<Vec<PresetTemplate>>,
    selected_id: ReadSignal<Option<String>>,
    has_editable_selection: Memo<bool>,
    on_select: impl Fn(web_sys::Event) + Clone + 'static,
    on_save: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_new: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_edit: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_delete: impl Fn(leptos::ev::MouseEvent) + 'static,
) -> impl IntoView {
    // Pre-compute optgroup visibility so the option closures don't
    // cross-subscribe to each other's data (which would recreate all
    // <option> elements any time either list changes).
    let show_optgroups =
        Memo::new(move |_| !bundled_presets.get().is_empty() && !effect_presets.get().is_empty());

    view! {
        <div class="flex items-center gap-2">
            // Preset selector dropdown
            <div class="flex-1 min-w-0">
                <select
                    class="w-full bg-surface-sunken/60 border border-edge-subtle rounded-lg px-2.5 py-1.5
                           text-xs text-fg-primary cursor-pointer truncate
                           focus:outline-none focus:border-accent-muted
                           transition-all duration-150"
                    on:change=on_select
                >
                    <option value="" selected=move || selected_id.get().is_none()>
                        "No preset"
                    </option>
                    // Bundled presets (effect-defined, read-only)
                    {move || {
                        let bp = bundled_presets.get();
                        if bp.is_empty() {
                            return view! { <></> }.into_any();
                        }
                        let groups = show_optgroups.get();
                        let options = bp.into_iter().enumerate().map(|(idx, p)| {
                            let val = format!("bundled:{idx}");
                            let option_value = val.clone();
                            let label = format!("\u{2726} {}", p.name);
                            view! {
                                <option
                                    value=val
                                    selected=move || selected_id.get().as_deref() == Some(option_value.as_str())
                                >
                                    {label}
                                </option>
                            }
                        }).collect_view();
                        if groups {
                            view! {
                                <optgroup label="Built-in">{options}</optgroup>
                            }.into_any()
                        } else {
                            options.into_any()
                        }
                    }}
                    // User-created presets
                    {move || {
                        let user = effect_presets.get();
                        if user.is_empty() {
                            return view! { <></> }.into_any();
                        }
                        let groups = show_optgroups.get();
                        let options = user.into_iter().map(|p| {
                            let id = p.id.clone();
                            let option_value = id.clone();
                            view! {
                                <option
                                    value=id
                                    selected=move || selected_id.get().as_deref() == Some(option_value.as_str())
                                >
                                    {p.name}
                                </option>
                            }
                        }).collect_view();
                        if groups {
                            view! {
                                <optgroup label="My Presets">{options}</optgroup>
                            }.into_any()
                        } else {
                            options.into_any()
                        }
                    }}
                </select>
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
                class="p-1.5 rounded-md text-fg-tertiary/50 transition-colors duration-150
                       hover:text-success-green hover:bg-success-green/10
                       disabled:opacity-20 disabled:cursor-not-allowed disabled:hover:bg-transparent disabled:hover:text-fg-tertiary/50"
                title="Save controls to preset"
                aria-label="Save controls to preset"
                disabled=move || !has_selection.get()
                on:click=on_save
            >
                <Icon icon=LuSave width="14px" height="14px" />
            </button>

            // New preset
            <button
                class="p-1.5 rounded-md text-fg-tertiary/50 transition-colors duration-150
                       hover:text-neon-cyan hover:bg-neon-cyan/10"
                title="Create new preset"
                aria-label="Create new preset"
                on:click=on_new
            >
                <Icon icon=LuPlus width="14px" height="14px" />
            </button>

            // Edit name
            <button
                class="p-1.5 rounded-md text-fg-tertiary/50 transition-colors duration-150
                       hover:text-electric-purple hover:bg-electric-purple/10
                       disabled:opacity-20 disabled:cursor-not-allowed disabled:hover:bg-transparent disabled:hover:text-fg-tertiary/50"
                title="Rename preset"
                aria-label="Rename preset"
                disabled=move || !has_selection.get()
                on:click=on_edit
            >
                <Icon icon=LuSquarePen width="14px" height="14px" />
            </button>

            // Delete
            <button
                class="p-1.5 rounded-md text-fg-tertiary/50 transition-colors duration-150
                       hover:text-error-red hover:bg-error-red/10
                       disabled:opacity-20 disabled:cursor-not-allowed disabled:hover:bg-transparent disabled:hover:text-fg-tertiary/50"
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
                class="flex-1 bg-surface-sunken/60 border border-accent-muted rounded-lg px-2.5 py-1.5
                       text-xs text-fg-primary placeholder-fg-tertiary/40
                       focus:outline-none focus:border-accent
                       transition-all duration-150"
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
            class="p-1.5 rounded-md text-fg-tertiary/50 transition-colors duration-150
                   hover:text-success-green hover:bg-success-green/10
                   disabled:opacity-20 disabled:cursor-not-allowed"
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
            class="p-1.5 rounded-md text-fg-tertiary/50 transition-colors duration-150
                   hover:text-error-red hover:bg-error-red/10"
            title="Cancel"
            on:click=move |_| on_cancel.run(())
        >
            <Icon icon=LuX width="14px" height="14px" />
        </button>
    }
}
