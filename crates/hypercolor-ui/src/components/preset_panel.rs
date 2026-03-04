//! Preset toolbar — compact single-line preset selector with save/create/edit/delete.

use leptos::prelude::*;
use std::collections::HashMap;
use wasm_bindgen::JsCast;

use hypercolor_types::effect::ControlValue;

use crate::api;

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
    accent_rgb: Signal<String>,
    /// Callback fired after a preset is applied (so parent can refresh controls).
    #[prop(into)]
    on_preset_applied: Callback<()>,
) -> impl IntoView {
    let (presets, set_presets) = signal(Vec::<api::PresetSummary>::new());
    let (selected_id, set_selected_id) = signal(Option::<String>::None);
    let (mode, set_mode) = signal(ToolbarMode::Idle);

    // Fetch presets whenever effect_id changes
    Effect::new(move |_| {
        let _eid = effect_id.get();
        set_selected_id.set(None);
        leptos::task::spawn_local(async move {
            match api::fetch_presets().await {
                Ok(all) => set_presets.set(all),
                Err(_) => set_presets.set(Vec::new()),
            }
        });
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

    let has_selection = Memo::new(move |_| selected_id.get().is_some());

    // Refresh helper
    let refresh_presets = move || {
        leptos::task::spawn_local(async move {
            if let Ok(all) = api::fetch_presets().await {
                set_presets.set(all);
            }
        });
    };

    // Select preset from dropdown → apply it
    let on_select = move |ev: web_sys::Event| {
        let target = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
        let Some(el) = target else { return };
        let val = el.value();
        if val.is_empty() {
            set_selected_id.set(None);
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
        let refresh = refresh_presets.clone();
        leptos::task::spawn_local(async move {
            let req = api::CreatePresetRequest {
                name,
                description: None,
                effect: eid,
                controls: serde_json::Value::Object(controls_json),
                tags: None,
            };
            if api::update_preset(&pid, &req).await.is_ok() {
                refresh();
            }
        });
    };

    // Create new preset
    let on_create = move |name: String| {
        let eid = effect_id.get().unwrap_or_default();
        let values = control_values.get();
        let controls_json = controls_to_json(&values);
        let refresh = refresh_presets.clone();
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
        let refresh = refresh_presets.clone();
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
        let refresh = refresh_presets.clone();
        set_selected_id.set(None);
        set_mode.set(ToolbarMode::Idle);
        leptos::task::spawn_local(async move {
            if api::delete_preset(&pid).await.is_ok() {
                refresh();
            }
        });
    };

    let accent_border = Signal::derive(move || {
        let rgb = accent_rgb.get();
        format!("border-color: rgba({rgb}, 0.12)")
    });

    view! {
        <div
            class="rounded-xl bg-layer-1 border border-white/[0.06] px-3 py-2.5
                   shadow-[0_2px_12px_rgba(0,0,0,0.15)]"
            style=move || accent_border.get()
        >
            {move || {
                match mode.get() {
                    ToolbarMode::Idle => {
                        let on_save = on_save.clone();
                        let on_delete = on_delete.clone();
                        view! {
                            <PresetSelectorRow
                                effect_presets=effect_presets
                                selected_id=selected_id
                                has_selection=has_selection
                                on_select=on_select.clone()
                                on_save=on_save
                                on_new=move |_| set_mode.set(ToolbarMode::Creating)
                                on_edit=move |_| set_mode.set(ToolbarMode::Renaming)
                                on_delete=on_delete
                            />
                        }.into_any()
                    }
                    ToolbarMode::Creating => {
                        let on_create = on_create.clone();
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
                        let on_rename = on_rename.clone();
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
    selected_id: ReadSignal<Option<String>>,
    has_selection: Memo<bool>,
    on_select: impl Fn(web_sys::Event) + Clone + 'static,
    on_save: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_new: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_edit: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_delete: impl Fn(leptos::ev::MouseEvent) + 'static,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2">
            // Preset selector dropdown
            <div class="flex-1 min-w-0">
                <select
                    class="w-full bg-layer-3/60 border border-white/[0.06] rounded-lg px-2.5 py-1.5
                           text-xs text-fg cursor-pointer truncate
                           focus:outline-none focus:border-electric-purple/20
                           transition-all duration-150"
                    on:change=on_select
                >
                    <option value="" selected=move || selected_id.get().is_none()>
                        "No preset"
                    </option>
                    {move || {
                        effect_presets.get().into_iter().map(|p| {
                            let id = p.id.clone();
                            let is_sel = {
                                let id = id.clone();
                                move || selected_id.get().as_deref() == Some(&id)
                            };
                            view! {
                                <option value=id selected=is_sel>{p.name}</option>
                            }
                        }).collect_view()
                    }}
                </select>
            </div>

            // Action buttons
            <PresetActionButtons
                has_selection=has_selection
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
                class="p-1.5 rounded-md text-fg-dim/50 transition-colors duration-150
                       hover:text-success-green hover:bg-success-green/10
                       disabled:opacity-20 disabled:cursor-not-allowed disabled:hover:bg-transparent disabled:hover:text-fg-dim/50"
                title="Save controls to preset"
                disabled=move || !has_selection.get()
                on:click=on_save
            >
                <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z" />
                    <polyline points="17 21 17 13 7 13 7 21" />
                    <polyline points="7 3 7 8 15 8" />
                </svg>
            </button>

            // New preset
            <button
                class="p-1.5 rounded-md text-fg-dim/50 transition-colors duration-150
                       hover:text-neon-cyan hover:bg-neon-cyan/10"
                title="Create new preset"
                on:click=on_new
            >
                <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <line x1="12" y1="5" x2="12" y2="19" />
                    <line x1="5" y1="12" x2="19" y2="12" />
                </svg>
            </button>

            // Edit name
            <button
                class="p-1.5 rounded-md text-fg-dim/50 transition-colors duration-150
                       hover:text-electric-purple hover:bg-electric-purple/10
                       disabled:opacity-20 disabled:cursor-not-allowed disabled:hover:bg-transparent disabled:hover:text-fg-dim/50"
                title="Rename preset"
                disabled=move || !has_selection.get()
                on:click=on_edit
            >
                <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" />
                    <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z" />
                </svg>
            </button>

            // Delete
            <button
                class="p-1.5 rounded-md text-fg-dim/50 transition-colors duration-150
                       hover:text-error-red hover:bg-error-red/10
                       disabled:opacity-20 disabled:cursor-not-allowed disabled:hover:bg-transparent disabled:hover:text-fg-dim/50"
                title="Delete preset"
                disabled=move || !has_selection.get()
                on:click=on_delete
            >
                <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="3 6 5 6 21 6" />
                    <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                </svg>
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
                class="flex-1 bg-layer-3/60 border border-electric-purple/20 rounded-lg px-2.5 py-1.5
                       text-xs text-fg placeholder-fg-dim/40
                       focus:outline-none focus:border-electric-purple/40
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
            class="p-1.5 rounded-md text-fg-dim/50 transition-colors duration-150
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
            <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none"
                 stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="20 6 9 17 4 12" />
            </svg>
        </button>
        <button
            class="p-1.5 rounded-md text-fg-dim/50 transition-colors duration-150
                   hover:text-error-red hover:bg-error-red/10"
            title="Cancel"
            on:click=move |_| on_cancel.run(())
        >
            <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none"
                 stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <line x1="18" y1="6" x2="6" y2="18" />
                <line x1="6" y1="6" x2="18" y2="18" />
            </svg>
        </button>
    }
}

/// Convert `ControlValue` map to JSON for the API.
fn controls_to_json(
    values: &HashMap<String, ControlValue>,
) -> serde_json::Map<String, serde_json::Value> {
    values
        .iter()
        .map(|(k, v)| (k.clone(), control_value_to_json(v)))
        .collect()
}

fn control_value_to_json(value: &ControlValue) -> serde_json::Value {
    match value {
        ControlValue::Float(v) => serde_json::json!(v),
        ControlValue::Integer(v) => serde_json::json!(v),
        ControlValue::Boolean(v) => serde_json::json!(v),
        ControlValue::Text(v) => serde_json::json!(v),
        ControlValue::Enum(v) => serde_json::json!(v),
        ControlValue::Color(v) => {
            serde_json::json!(format!(
                "#{:02x}{:02x}{:02x}",
                (v[0] * 255.0) as u8,
                (v[1] * 255.0) as u8,
                (v[2] * 255.0) as u8,
            ))
        }
        ControlValue::Gradient(stops) => serde_json::json!(stops),
    }
}
