//! Reusable setting control components for the Settings page.

use hypercolor_leptos_ext::events::{Change, Input};
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::components::silk_select::SilkSelect;
use crate::icons::*;

// ── Section Header ─────────────────────────────────────────────────────────

/// Large section header used between settings groups. Uses the canonical
/// `Section` size from the shared label system with a purple icon accent.
#[component]
pub fn SectionHeader(title: &'static str, icon: icondata_core::Icon) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2.5 mb-5">
            <Icon icon=icon width="16px" height="16px" style="color: rgba(225, 53, 255, 0.6)" />
            <h2 class=label_class(LabelSize::Section, LabelTone::Default)>{title}</h2>
        </div>
    }
}

// ── Restart Badge ──────────────────────────────────────────────────────────

fn restart_badge() -> impl IntoView {
    view! {
        <span
            class="text-[9px] font-mono px-1.5 py-0.5 rounded"
            style="color: rgba(241, 250, 140, 0.7); background: rgba(241, 250, 140, 0.08)"
        >
            "restart"
        </span>
    }
}

// ── Setting Toggle ─────────────────────────────────────────────────────────

#[component]
pub fn SettingToggle(
    label: &'static str,
    description: &'static str,
    key: &'static str,
    #[prop(into)] value: Signal<bool>,
    on_change: Callback<(String, serde_json::Value)>,
    #[prop(default = false)] restart_required: bool,
) -> impl IntoView {
    let key_owned = key.to_string();
    view! {
        <div class="flex items-start justify-between gap-4 py-3 setting-row">
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                    <span class="text-sm text-fg-primary font-medium">{label}</span>
                    {restart_required.then(restart_badge)}
                </div>
                <div class="text-xs text-fg-tertiary/70 mt-0.5">{description}</div>
            </div>
            <button
                role="switch"
                aria-checked=move || value.get().to_string()
                class="relative w-11 h-6 rounded-full transition-all duration-200 shrink-0 mt-0.5 cursor-pointer"
                style=move || if value.get() {
                    "background: rgba(225, 53, 255, 0.5); box-shadow: 0 0 10px rgba(225, 53, 255, 0.25)"
                } else {
                    "background: rgba(139, 133, 160, 0.2)"
                }
                on:click=move |_| {
                    on_change.run((key_owned.clone(), serde_json::json!(!value.get())));
                }
            >
                <span
                    class="absolute left-0.5 top-0.5 w-5 h-5 rounded-full shadow-sm transition-transform duration-200"
                    style=move || if value.get() {
                        "transform: translateX(22px); background: rgb(225, 53, 255)"
                    } else {
                        "transform: translateX(0); background: rgba(200, 200, 210, 0.6)"
                    }
                />
            </button>
        </div>
    }
}

// ── Setting Slider ─────────────────────────────────────────────────────────

#[component]
pub fn SettingSlider(
    label: &'static str,
    description: &'static str,
    key: &'static str,
    #[prop(into)] value: Signal<f64>,
    on_change: Callback<(String, serde_json::Value)>,
    min: f64,
    max: f64,
    step: f64,
    #[prop(default = false)] restart_required: bool,
    #[prop(default = 2)] decimals: usize,
    #[prop(default = false)] integer: bool,
) -> impl IntoView {
    let key_owned = key.to_string();
    let fmt = move || {
        let v = value.get();
        if decimals == 0 {
            format!("{v:.0}")
        } else {
            format!("{v:.prec$}", prec = decimals)
        }
    };
    view! {
        <div class="flex items-start justify-between gap-4 py-3 setting-row">
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                    <span class="text-sm text-fg-primary font-medium">{label}</span>
                    {restart_required.then(restart_badge)}
                </div>
                <div class="text-xs text-fg-tertiary/70 mt-0.5">{description}</div>
            </div>
            <div class="flex items-center gap-3 shrink-0">
                <input
                    type="range"
                    class="w-28 h-1 rounded-full appearance-none cursor-pointer"
                    style="accent-color: rgb(225, 53, 255); background: rgba(139, 133, 160, 0.15)"
                    prop:value=move || value.get().to_string()
                    min=min.to_string()
                    max=max.to_string()
                    step=step.to_string()
                    on:change=move |ev| {
                        let event = Change::from_event(ev);
                        if let Some(val) = event.value::<f64>() {
                            let json_val = if integer {
                                serde_json::json!(val as i64)
                            } else {
                                serde_json::json!(val)
                            };
                            on_change.run((key_owned.clone(), json_val));
                        }
                    }
                />
                <span class="text-xs font-mono text-fg-tertiary tabular-nums w-12 text-right">
                    {fmt}
                </span>
            </div>
        </div>
    }
}

// ── Setting Segmented ──────────────────────────────────────────────────────

/// A segmented-button picker for discrete presets. Values are strings so it can
/// back either textual or numeric keys (numeric values should be stringified in
/// the `options` slot and the caller should set `numeric=true`).
#[component]
pub fn SettingSegmented(
    label: &'static str,
    description: &'static str,
    key: &'static str,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] options: Signal<Vec<(String, String)>>,
    on_change: Callback<(String, serde_json::Value)>,
    #[prop(default = false)] restart_required: bool,
    #[prop(default = false)] numeric: bool,
) -> impl IntoView {
    let key_owned = key.to_string();
    view! {
        <div class="flex items-start justify-between gap-4 py-3 setting-row">
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                    <span class="text-sm text-fg-primary font-medium">{label}</span>
                    {restart_required.then(restart_badge)}
                </div>
                <div class="text-xs text-fg-tertiary/70 mt-0.5">{description}</div>
            </div>
            <div
                class="flex items-center gap-0.5 p-0.5 rounded-lg shrink-0"
                style="background: rgba(139, 133, 160, 0.08); border: 1px solid rgba(139, 133, 160, 0.12)"
            >
                {move || {
                    let current = value.get();
                    options.get().into_iter().map(|(val, label)| {
                        let is_selected = current == val;
                        let key_click = key_owned.clone();
                        let val_click = val.clone();
                        view! {
                            <button
                                class="px-2.5 py-1 text-xs font-mono rounded-md transition-all duration-150 cursor-pointer tabular-nums"
                                style=move || if is_selected {
                                    "color: rgb(230, 237, 243); background: rgba(225, 53, 255, 0.18); box-shadow: 0 0 8px rgba(225, 53, 255, 0.15)"
                                } else {
                                    "color: rgba(139, 133, 160, 0.7); background: transparent"
                                }
                                on:click=move |_| {
                                    let json_val = if numeric {
                                        val_click.parse::<i64>()
                                            .map(|n| serde_json::json!(n))
                                            .unwrap_or_else(|_| serde_json::json!(val_click))
                                    } else {
                                        serde_json::json!(val_click)
                                    };
                                    on_change.run((key_click.clone(), json_val));
                                }
                            >
                                {label}
                            </button>
                        }
                    }).collect_view()
                }}
            </div>
        </div>
    }
}

// ── Setting Dropdown ───────────────────────────────────────────────────────

#[component]
pub fn SettingDropdown(
    label: &'static str,
    description: &'static str,
    key: &'static str,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] options: Signal<Vec<(String, String)>>,
    on_change: Callback<(String, serde_json::Value)>,
    #[prop(into, optional)] placeholder: MaybeProp<String>,
    #[prop(into, optional)] disabled: MaybeProp<bool>,
    #[prop(default = false)] restart_required: bool,
    #[prop(default = false)] numeric: bool,
) -> impl IntoView {
    let key_owned = key.to_string();
    view! {
        <div class="flex flex-col gap-3 py-3 setting-row md:flex-row md:items-start md:justify-between">
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                    <span class="text-sm text-fg-primary font-medium">{label}</span>
                    {restart_required.then(restart_badge)}
                </div>
                <div class="text-xs text-fg-tertiary/70 mt-0.5">{description}</div>
            </div>
            <div class="w-full md:w-auto md:min-w-[16rem] md:max-w-[30rem] md:shrink-0">
                <SilkSelect
                    value=value
                    options=options
                    placeholder=placeholder
                    disabled=disabled
                    on_change=Callback::new(move |str_val: String| {
                        let json_val = if numeric {
                            str_val.parse::<i64>()
                                .map(|n| serde_json::json!(n))
                                .unwrap_or_else(|_| serde_json::json!(str_val))
                        } else {
                            serde_json::json!(str_val)
                        };
                        on_change.run((key_owned.clone(), json_val));
                    })
                    class="bg-surface-overlay/60 border border-edge-subtle px-3 py-1.5 text-sm text-fg-primary"
                />
            </div>
        </div>
    }
}

// ── Setting Text Input ─────────────────────────────────────────────────────

#[component]
pub fn SettingTextInput(
    label: &'static str,
    description: &'static str,
    key: &'static str,
    #[prop(into)] value: Signal<String>,
    on_change: Callback<(String, serde_json::Value)>,
    #[prop(default = false)] restart_required: bool,
    #[prop(default = "")] placeholder: &'static str,
) -> impl IntoView {
    let key_owned = key.to_string();
    view! {
        <div class="flex items-start justify-between gap-4 py-3 setting-row">
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                    <span class="text-sm text-fg-primary font-medium">{label}</span>
                    {restart_required.then(restart_badge)}
                </div>
                <div class="text-xs text-fg-tertiary/70 mt-0.5">{description}</div>
            </div>
            <input
                type="text"
                class="bg-surface-overlay/60 border border-edge-subtle rounded-lg px-3 py-1.5 text-sm text-fg-primary
                       placeholder-fg-tertiary/40 focus:outline-none focus:border-accent-muted shrink-0 w-48"
                prop:value=move || value.get()
                placeholder=placeholder
                on:change=move |ev| {
                    let event = Change::from_event(ev);
                    if let Some(value) = event.value_string() {
                        on_change.run((key_owned.clone(), serde_json::json!(value)));
                    }
                }
            />
        </div>
    }
}

// ── Setting Number Input ───────────────────────────────────────────────────

#[component]
pub fn SettingNumberInput(
    label: &'static str,
    description: &'static str,
    key: &'static str,
    #[prop(into)] value: Signal<f64>,
    on_change: Callback<(String, serde_json::Value)>,
    min: f64,
    max: f64,
    step: f64,
    #[prop(default = false)] restart_required: bool,
    #[prop(default = true)] integer: bool,
) -> impl IntoView {
    let key_owned = key.to_string();
    let key_dec = key.to_string();
    let key_inc = key.to_string();
    let commit = move |v: f64| {
        let clamped = v.clamp(min, max);
        if integer {
            serde_json::json!(clamped as i64)
        } else {
            serde_json::json!(clamped)
        }
    };
    view! {
        <div class="flex items-start justify-between gap-4 py-3 setting-row">
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                    <span class="text-sm text-fg-primary font-medium">{label}</span>
                    {restart_required.then(restart_badge)}
                </div>
                <div class="text-xs text-fg-tertiary/70 mt-0.5">{description}</div>
            </div>
            <div class="flex items-center gap-1 shrink-0">
                <button
                    class="w-7 h-7 rounded-md flex items-center justify-center text-fg-tertiary
                           hover:text-fg-primary hover:bg-surface-hover/40 transition-colors"
                    on:click=move |_| {
                        let val = commit(value.get() - step);
                        on_change.run((key_dec.clone(), val));
                    }
                >
                    <Icon icon=LuMinus width="12px" height="12px" />
                </button>
                <input
                    type="number"
                    class="bg-surface-overlay/60 border border-edge-subtle rounded-lg px-2 py-1 text-sm text-fg-primary
                           text-center font-mono tabular-nums w-20 focus:outline-none focus:border-accent-muted
                           [appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
                    prop:value=move || {
                        if integer { format!("{:.0}", value.get()) } else { format!("{}", value.get()) }
                    }
                    min=min.to_string()
                    max=max.to_string()
                    step=step.to_string()
                    on:change=move |ev| {
                        let event = Change::from_event(ev);
                        if let Some(v) = event.value::<f64>() {
                            let val = commit(v);
                            on_change.run((key_owned.clone(), val));
                        }
                    }
                />
                <button
                    class="w-7 h-7 rounded-md flex items-center justify-center text-fg-tertiary
                           hover:text-fg-primary hover:bg-surface-hover/40 transition-colors"
                    on:click=move |_| {
                        let val = commit(value.get() + step);
                        on_change.run((key_inc.clone(), val));
                    }
                >
                    <Icon icon=LuPlus width="12px" height="12px" />
                </button>
            </div>
        </div>
    }
}

// ── Setting Path List ──────────────────────────────────────────────────────

#[component]
pub fn SettingPathList(
    label: &'static str,
    description: &'static str,
    key: &'static str,
    #[prop(into)] paths: Signal<Vec<String>>,
    on_change: Callback<(String, serde_json::Value)>,
) -> impl IntoView {
    let key_owned = key.to_string();
    let key_add = key.to_string();
    let key_add_btn = key.to_string();
    let (new_path, set_new_path) = signal(String::new());

    view! {
        <div class="py-3 setting-row">
            <div class="mb-3">
                <div class="flex items-center gap-2">
                    <span class="text-sm text-fg-primary font-medium">{label}</span>
                </div>
                <div class="text-xs text-fg-tertiary/70 mt-0.5">{description}</div>
            </div>

            // Current paths
            <div class="space-y-1.5 mb-3">
                {move || {
                    let current_paths = paths.get();
                    if current_paths.is_empty() {
                        view! {
                            <div class="text-xs text-fg-tertiary/50 italic px-3 py-2">"No custom directories"</div>
                        }.into_any()
                    } else {
                        let key_for_remove = key_owned.clone();
                        current_paths.into_iter().enumerate().map(move |(i, path)| {
                            let key_rm = key_for_remove.clone();
                            view! {
                                <div class="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-surface-overlay/30 group">
                                    <Icon icon=LuFolder width="13px" height="13px" style="color: rgba(128, 255, 234, 0.5)" />
                                    <span class="text-xs text-fg-secondary font-mono flex-1 truncate">{path}</span>
                                    <button
                                        class="w-5 h-5 rounded flex items-center justify-center text-fg-tertiary/30
                                               hover:text-error-red transition-colors opacity-0 group-hover:opacity-100"
                                        on:click=move |_| {
                                            let mut updated = paths.get();
                                            if i < updated.len() {
                                                updated.remove(i);
                                                on_change.run((key_rm.clone(), serde_json::json!(updated)));
                                            }
                                        }
                                    >
                                        <Icon icon=LuX width="12px" height="12px" />
                                    </button>
                                </div>
                            }
                        }).collect_view().into_any()
                    }
                }}
            </div>

            // Add new path
            <div class="flex items-center gap-2">
                <input
                    type="text"
                    class="flex-1 bg-surface-overlay/40 border border-edge-subtle rounded-lg px-3 py-1.5 text-xs text-fg-primary
                           font-mono placeholder-fg-tertiary/30 focus:outline-none focus:border-accent-muted"
                    placeholder="/path/to/effects/directory"
                    prop:value=move || new_path.get()
                    on:input=move |ev| {
                        let event = Input::from_event(ev);
                        if let Some(value) = event.value_string() {
                            set_new_path.set(value);
                        }
                    }
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" {
                            let path = new_path.get().trim().to_string();
                            if !path.is_empty() {
                                let mut current = paths.get();
                                current.push(path);
                                on_change.run((key_add.clone(), serde_json::json!(current)));
                                set_new_path.set(String::new());
                            }
                        }
                    }
                />
                <button
                    class="w-7 h-7 rounded-lg flex items-center justify-center border border-edge-subtle
                           text-fg-tertiary hover:text-accent hover:border-accent-muted transition-colors"
                    on:click=move |_| {
                        let path = new_path.get().trim().to_string();
                        if !path.is_empty() {
                            let mut current = paths.get();
                            current.push(path);
                            on_change.run((key_add_btn.clone(), serde_json::json!(current)));
                            set_new_path.set(String::new());
                        }
                    }
                >
                    <Icon icon=LuPlus width="14px" height="14px" />
                </button>
            </div>
        </div>
    }
}

// ── Section Reset ──────────────────────────────────────────────────────────

#[component]
pub fn SectionReset(section_label: &'static str, on_reset: Callback<()>) -> impl IntoView {
    let (confirming, set_confirming) = signal(false);
    view! {
        <div class="pt-3 mt-1">
            {move || if confirming.get() {
                view! {
                    <div class="flex items-center gap-3">
                        <span class="text-xs text-fg-tertiary">
                            {format!("Reset all {section_label} settings to defaults?")}
                        </span>
                        <button
                            class="text-xs px-2.5 py-1 rounded-md font-medium transition-colors"
                            style="color: rgb(255, 99, 99); background: rgba(255, 99, 99, 0.1)"
                            on:click=move |_| {
                                on_reset.run(());
                                set_confirming.set(false);
                            }
                        >
                            "Reset"
                        </button>
                        <button
                            class="text-xs px-2.5 py-1 rounded-md text-fg-tertiary hover:text-fg-secondary transition-colors"
                            on:click=move |_| set_confirming.set(false)
                        >
                            "Cancel"
                        </button>
                    </div>
                }.into_any()
            } else {
                view! {
                    <button
                        class="flex items-center gap-1.5 text-xs text-fg-tertiary/50 hover:text-fg-tertiary transition-colors"
                        on:click=move |_| set_confirming.set(true)
                    >
                        <Icon icon=LuUndo2 width="11px" height="11px" />
                        {format!("Reset {section_label} to defaults")}
                    </button>
                }.into_any()
            }}
        </div>
    }
}
