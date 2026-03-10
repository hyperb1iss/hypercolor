//! Reusable setting control components for the Settings page.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::icons::*;

// ── Section Header ─────────────────────────────────────────────────────────

#[component]
pub fn SectionHeader(
    title: &'static str,
    description: &'static str,
    icon: icondata_core::Icon,
) -> impl IntoView {
    view! {
        <div class="flex items-start gap-3.5 mb-5">
            <div class="section-icon-shell shrink-0">
                <Icon icon=icon width="16px" height="16px" style="color: rgba(128, 255, 234, 0.9)" />
            </div>
            <div class="min-w-0 space-y-1">
                <div class="text-[10px] font-mono uppercase tracking-[0.18em] text-electric-purple/70">
                    "Configuration"
                </div>
                <div class="space-y-1">
                    <h2 class="text-base font-medium text-fg-primary">{title}</h2>
                    <p class="text-xs leading-relaxed text-fg-tertiary/70 max-w-2xl">{description}</p>
                </div>
            </div>
        </div>
    }
}

#[component]
pub fn SettingGroupHeading(title: &'static str, description: &'static str) -> impl IntoView {
    view! {
        <div class="settings-group-heading">
            <div>
                <div class="text-[10px] font-mono uppercase tracking-[0.16em] text-neon-cyan/75">
                    {title}
                </div>
                <p class="mt-1 text-xs text-fg-tertiary/65 leading-relaxed">{description}</p>
            </div>
        </div>
    }
}

// ── Badges ─────────────────────────────────────────────────────────────────

fn restart_badge() -> impl IntoView {
    view! { <span class="setting-badge setting-badge-restart">"restart"</span> }
}

fn live_badge() -> impl IntoView {
    view! { <span class="setting-badge setting-badge-live">"live"</span> }
}

// ── Shared Shell ───────────────────────────────────────────────────────────

#[component]
fn SettingShell(
    label: &'static str,
    description: &'static str,
    icon: icondata_core::Icon,
    #[prop(default = false)] restart_required: bool,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="py-3 setting-row">
            <div class="setting-card">
                <div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
                    <div class="flex items-start gap-3 min-w-0">
                        <div class="setting-icon-chip shrink-0">
                            <Icon icon=icon width="14px" height="14px" style="color: rgba(128, 255, 234, 0.9)" />
                        </div>
                        <div class="min-w-0">
                            <div class="flex flex-wrap items-center gap-2">
                                <span class="text-sm font-medium text-fg-primary">{label}</span>
                                {if restart_required {
                                    restart_badge().into_any()
                                } else {
                                    live_badge().into_any()
                                }}
                            </div>
                            <p class="mt-1 text-xs leading-relaxed text-fg-tertiary/70">{description}</p>
                        </div>
                    </div>
                    <div class="flex items-center justify-end shrink-0 lg:pl-6">
                        {children()}
                    </div>
                </div>
            </div>
        </div>
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
        <SettingShell
            label=label
            description=description
            icon=LuPower
            restart_required=restart_required
        >
            <button
                type="button"
                role="switch"
                aria-checked=move || value.get().to_string()
                class="toggle-track relative w-11 h-6 rounded-full shrink-0 cursor-pointer border border-white/6 overflow-hidden"
                style=move || if value.get() {
                    "background: linear-gradient(135deg, rgba(225, 53, 255, 0.85), rgba(255, 106, 193, 0.72)); box-shadow: 0 0 18px rgba(225, 53, 255, 0.22), inset 0 0 10px rgba(255, 255, 255, 0.06)"
                } else {
                    "background: rgba(139, 133, 160, 0.14); box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.03)"
                }
                on:click=move |_| {
                    on_change.run((key_owned.clone(), serde_json::json!(!value.get())));
                }
            >
                <span
                    class="toggle-thumb absolute left-[2px] top-[2px] w-5 h-5 rounded-full shadow-sm"
                    style=move || if value.get() {
                        "transform: translateX(20px); background: linear-gradient(180deg, rgba(255,255,255,0.98), rgba(255,255,255,0.82)); box-shadow: 0 0 14px rgba(255, 255, 255, 0.3)"
                    } else {
                        "transform: translateX(0); background: rgba(214, 216, 226, 0.82)"
                    }
                />
            </button>
        </SettingShell>
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
        <SettingShell
            label=label
            description=description
            icon=LuCircleDot
            restart_required=restart_required
        >
            <div class="flex items-center gap-3 shrink-0 min-w-[15rem]">
                <input
                    type="range"
                    class="w-32 h-1 rounded-full appearance-none cursor-pointer"
                    style="accent-color: rgb(225, 53, 255); background: rgba(139, 133, 160, 0.15)"
                    prop:value=move || value.get().to_string()
                    min=min.to_string()
                    max=max.to_string()
                    step=step.to_string()
                    on:change=move |ev| {
                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                        if let Some(el) = target {
                            if let Ok(val) = el.value().parse::<f64>() {
                                let json_val = if integer {
                                    serde_json::json!(val as i64)
                                } else {
                                    serde_json::json!(val)
                                };
                                on_change.run((key_owned.clone(), json_val));
                            }
                        }
                    }
                />
                <span class="setting-value-pill text-xs font-mono tabular-nums w-14 text-center">
                    {fmt}
                </span>
            </div>
        </SettingShell>
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
    #[prop(default = false)] numeric: bool,
) -> impl IntoView {
    let key_owned = key.to_string();
    view! {
        <SettingShell
            label=label
            description=description
            icon=LuSettings2
            restart_required=restart_required
        >
            <select
                class="setting-select bg-surface-overlay/60 border border-edge-subtle rounded-xl px-3.5 py-2 text-sm text-fg-primary
                       focus:outline-none focus:border-accent-muted cursor-pointer shrink-0 min-w-[160px]"
                prop:value=move || value.get()
                on:change=move |ev| {
                    let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
                    if let Some(el) = target {
                        let str_val = el.value();
                        let json_val = if numeric {
                            str_val
                                .parse::<i64>()
                                .map(|n| serde_json::json!(n))
                                .unwrap_or_else(|_| serde_json::json!(str_val))
                        } else {
                            serde_json::json!(str_val)
                        };
                        on_change.run((key_owned.clone(), json_val));
                    }
                }
            >
                {move || options.get().into_iter().map(|(val, label)| {
                    let is_selected = value.get() == val;
                    view! { <option value=val selected=is_selected>{label}</option> }
                }).collect_view()}
            </select>
        </SettingShell>
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
        <SettingShell
            label=label
            description=description
            icon=LuCode
            restart_required=restart_required
        >
            <input
                type="text"
                class="setting-input bg-surface-overlay/60 border border-edge-subtle rounded-xl px-3.5 py-2 text-sm text-fg-primary
                       placeholder-fg-tertiary/40 focus:outline-none focus:border-accent-muted shrink-0 w-56"
                prop:value=move || value.get()
                placeholder=placeholder
                on:change=move |ev| {
                    let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                    if let Some(el) = target {
                        on_change.run((key_owned.clone(), serde_json::json!(el.value())));
                    }
                }
            />
        </SettingShell>
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
        <SettingShell
            label=label
            description=description
            icon=LuCircleDot
            restart_required=restart_required
        >
            <div class="flex items-center gap-1.5 shrink-0">
                <button
                    type="button"
                    class="setting-stepper-btn w-8 h-8 rounded-lg flex items-center justify-center text-fg-tertiary"
                    on:click=move |_| {
                        let val = commit(value.get() - step);
                        on_change.run((key_dec.clone(), val));
                    }
                >
                    <Icon icon=LuMinus width="12px" height="12px" />
                </button>
                <input
                    type="number"
                    class="setting-input bg-surface-overlay/60 border border-edge-subtle rounded-xl px-2.5 py-2 text-sm text-fg-primary
                           text-center font-mono tabular-nums w-24 focus:outline-none focus:border-accent-muted
                           [appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
                    prop:value=move || {
                        if integer {
                            format!("{:.0}", value.get())
                        } else {
                            format!("{}", value.get())
                        }
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
                    type="button"
                    class="setting-stepper-btn w-8 h-8 rounded-lg flex items-center justify-center text-fg-tertiary"
                    on:click=move |_| {
                        let val = commit(value.get() + step);
                        on_change.run((key_inc.clone(), val));
                    }
                >
                    <Icon icon=LuPlus width="12px" height="12px" />
                </button>
            </div>
        </SettingShell>
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
            <div class="setting-card setting-card-stack">
                <div class="flex items-start gap-3">
                    <div class="setting-icon-chip shrink-0">
                        <Icon icon=LuFolder width="14px" height="14px" style="color: rgba(128, 255, 234, 0.9)" />
                    </div>
                    <div class="min-w-0">
                        <div class="flex flex-wrap items-center gap-2">
                            <span class="text-sm font-medium text-fg-primary">{label}</span>
                            {live_badge()}
                        </div>
                        <p class="mt-1 text-xs leading-relaxed text-fg-tertiary/70">{description}</p>
                    </div>
                </div>

                <div class="space-y-2.5 mt-4">
                    {move || {
                        let current_paths = paths.get();
                        if current_paths.is_empty() {
                            view! {
                                <div class="rounded-xl border border-dashed border-white/6 bg-white/[0.02] px-3 py-3 text-xs italic text-fg-tertiary/50">
                                    "No custom directories yet."
                                </div>
                            }.into_any()
                        } else {
                            let key_for_remove = key_owned.clone();
                            current_paths.into_iter().enumerate().map(move |(i, path)| {
                                let key_rm = key_for_remove.clone();
                                view! {
                                    <div class="flex items-center gap-2 px-3 py-2 rounded-xl border border-white/6 bg-white/[0.025] group">
                                        <Icon icon=LuFolder width="13px" height="13px" style="color: rgba(128, 255, 234, 0.55)" />
                                        <span class="text-xs text-fg-secondary font-mono flex-1 truncate">{path}</span>
                                        <button
                                            type="button"
                                            class="w-6 h-6 rounded-lg flex items-center justify-center text-fg-tertiary/45
                                                   hover:text-error-red hover:bg-white/[0.04] transition-colors opacity-0 group-hover:opacity-100"
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

                <div class="flex flex-col gap-2 mt-4 md:flex-row">
                    <input
                        type="text"
                        class="setting-input flex-1 bg-surface-overlay/40 border border-edge-subtle rounded-xl px-3.5 py-2 text-xs text-fg-primary
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
                        type="button"
                        class="setting-stepper-btn h-10 px-3 rounded-xl inline-flex items-center justify-center gap-1.5 border border-edge-subtle
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
                        <span class="text-xs font-medium">"Add directory"</span>
                    </button>
                </div>
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
                    <div class="flex flex-wrap items-center gap-3 rounded-xl border border-error-red/20 bg-error-red/6 px-3 py-2.5">
                        <span class="text-xs text-fg-tertiary leading-relaxed">
                            {format!("Reset all {section_label} settings to defaults?")}
                        </span>
                        <button
                            type="button"
                            class="text-xs px-3 py-1.5 rounded-lg font-medium transition-colors"
                            style="color: rgb(255, 99, 99); background: rgba(255, 99, 99, 0.12)"
                            on:click=move |_| {
                                on_reset.run(());
                                set_confirming.set(false);
                            }
                        >
                            "Reset"
                        </button>
                        <button
                            type="button"
                            class="text-xs px-3 py-1.5 rounded-lg text-fg-tertiary hover:text-fg-secondary hover:bg-white/[0.04] transition-colors"
                            on:click=move |_| set_confirming.set(false)
                        >
                            "Cancel"
                        </button>
                    </div>
                }.into_any()
            } else {
                view! {
                    <button
                        type="button"
                        class="inline-flex items-center gap-1.5 text-xs text-fg-tertiary/55 hover:text-fg-tertiary transition-colors"
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
