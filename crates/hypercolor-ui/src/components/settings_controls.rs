//! Reusable setting control components for the Settings page.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::icons::*;

// ── Section Header ─────────────────────────────────────────────────────────

#[component]
pub fn SectionHeader(title: &'static str, icon: icondata_core::Icon) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2.5 mb-4">
            <Icon icon=icon width="18px" height="18px" style="color: rgba(225, 53, 255, 0.7)" />
            <h2 class="text-sm font-mono uppercase tracking-[0.1em] text-fg-secondary">{title}</h2>
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
                    {restart_required.then(|| restart_badge())}
                </div>
                <div class="text-xs text-fg-tertiary/70 mt-0.5">{description}</div>
            </div>
            <button
                role="switch"
                aria-checked=move || value.get().to_string()
                class="relative w-9 h-5 rounded-full transition-all duration-200 shrink-0 mt-0.5 cursor-pointer"
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
                    class="absolute top-0.5 w-4 h-4 rounded-full shadow-sm transition-transform duration-200"
                    style=move || if value.get() {
                        "transform: translateX(18px); background: rgb(225, 53, 255)"
                    } else {
                        "transform: translateX(2px); background: rgba(200, 200, 210, 0.6)"
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
                    {restart_required.then(|| restart_badge())}
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
                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(el) = target {
                            if let Ok(val) = el.value().parse::<f64>() {
                                on_change.run((key_owned.clone(), serde_json::json!(val)));
                            }
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

// ── Setting Dropdown ───────────────────────────────────────────────────────

#[component]
pub fn SettingDropdown(
    label: &'static str,
    description: &'static str,
    key: &'static str,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] options: Signal<Vec<(String, String)>>,
    on_change: Callback<(String, serde_json::Value)>,
    #[prop(default = false)] restart_required: bool,
) -> impl IntoView {
    let key_owned = key.to_string();
    view! {
        <div class="flex items-start justify-between gap-4 py-3 setting-row">
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                    <span class="text-sm text-fg-primary font-medium">{label}</span>
                    {restart_required.then(|| restart_badge())}
                </div>
                <div class="text-xs text-fg-tertiary/70 mt-0.5">{description}</div>
            </div>
            <select
                class="bg-surface-overlay/60 border border-edge-subtle rounded-lg px-3 py-1.5 text-sm text-fg-primary
                       focus:outline-none focus:border-accent-muted cursor-pointer shrink-0 min-w-[120px]"
                prop:value=move || value.get()
                on:change=move |ev| {
                    let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
                    if let Some(el) = target {
                        on_change.run((key_owned.clone(), serde_json::json!(el.value())));
                    }
                }
            >
                {move || options.get().into_iter().map(|(val, label)| {
                    let is_selected = value.get() == val;
                    view! { <option value=val selected=is_selected>{label}</option> }
                }).collect_view()}
            </select>
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
                    {restart_required.then(|| restart_badge())}
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
                    let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                    if let Some(el) = target {
                        on_change.run((key_owned.clone(), serde_json::json!(el.value())));
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
        let val = if integer {
            serde_json::json!(clamped as i64)
        } else {
            serde_json::json!(clamped)
        };
        val
    };
    view! {
        <div class="flex items-start justify-between gap-4 py-3 setting-row">
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                    <span class="text-sm text-fg-primary font-medium">{label}</span>
                    {restart_required.then(|| restart_badge())}
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
                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(el) = target {
                            if let Ok(v) = el.value().parse::<f64>() {
                                let val = commit(v);
                                on_change.run((key_owned.clone(), val));
                            }
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
                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(el) = target {
                            set_new_path.set(el.value());
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
pub fn SectionReset(
    section_label: &'static str,
    on_reset: Callback<()>,
) -> impl IntoView {
    let (confirming, set_confirming) = signal(false);
    view! {
        <div class="pt-4 mt-2 border-t border-edge-subtle/20">
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
