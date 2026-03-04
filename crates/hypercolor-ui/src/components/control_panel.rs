//! Auto-generated control panel — renders widgets from ControlDefinition metadata.
//! Each control resolves its initial value from live `control_values` (if present),
//! falling back to the definition's `default_value`.

use leptos::prelude::*;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};

use hypercolor_types::effect::{ControlDefinition, ControlType, ControlValue};

/// Resolve the effective value for a control: live value > default.
fn effective_value(
    def: &ControlDefinition,
    live_values: &HashMap<String, ControlValue>,
) -> ControlValue {
    live_values
        .get(def.control_id())
        .cloned()
        .unwrap_or_else(|| def.default_value.clone())
}

/// Auto-generated control panel for the active effect.
#[component]
pub fn ControlPanel(
    #[prop(into)] controls: Signal<Vec<ControlDefinition>>,
    #[prop(into)] control_values: Signal<HashMap<String, ControlValue>>,
    #[prop(into)] accent_rgb: Signal<String>,
    #[prop(into)] on_change: Callback<(String, serde_json::Value)>,
) -> impl IntoView {
    let grouped = Memo::new(move |_| {
        let defs = controls.get();
        let values = control_values.get();
        let rgb = accent_rgb.get();
        let mut groups: BTreeMap<String, Vec<(ControlDefinition, ControlValue, String)>> =
            BTreeMap::new();
        for def in defs {
            let value = effective_value(&def, &values);
            let group = def.group.clone().unwrap_or_else(|| "General".to_string());
            groups
                .entry(group)
                .or_default()
                .push((def, value, rgb.clone()));
        }
        groups
    });

    view! {
        <div class="space-y-5">
            {move || {
                let groups = grouped.get();
                if groups.is_empty() {
                    view! {
                        <div class="text-center py-6">
                            <div class="text-fg-dim/50 text-xs">"No controls available"</div>
                        </div>
                    }.into_any()
                } else {
                    groups.into_iter().map(|(group, items)| {
                        view! {
                            <div class="space-y-3">
                                <div class="flex items-center gap-2">
                                    <div class="h-px flex-1 bg-white/[0.04]" />
                                    <h4 class="text-[9px] font-mono uppercase tracking-[0.2em] text-fg-dim/60 shrink-0">
                                        {group}
                                    </h4>
                                    <div class="h-px flex-1 bg-white/[0.04]" />
                                </div>
                                <div class="space-y-1">
                                    {items.into_iter().map(|(def, value, rgb)| {
                                        view! { <ControlWidget def=def initial_value=value accent_rgb=rgb on_change=on_change /> }
                                    }).collect_view()}
                                </div>
                            </div>
                        }
                    }).collect_view().into_any()
                }
            }}
        </div>
    }
}

/// A single control widget, dispatched by ControlType.
#[component]
fn ControlWidget(
    def: ControlDefinition,
    initial_value: ControlValue,
    accent_rgb: String,
    on_change: Callback<(String, serde_json::Value)>,
) -> impl IntoView {
    let name = def.name.clone();
    let control_id = def.control_id().to_owned();
    let tooltip = def.tooltip.clone();

    match def.control_type {
        ControlType::Slider => {
            let initial = initial_value.as_f32().unwrap_or(0.5);
            let min = def.min.unwrap_or(0.0);
            let max = def.max.unwrap_or(1.0);
            let step = def.step.unwrap_or(0.01);
            let (value, set_value) = signal(initial);
            let control_name = control_id.clone();

            // Accent-colored value badge
            let badge_style = format!(
                "color: rgba({}, 0.9); background: rgba({}, 0.08)",
                accent_rgb, accent_rgb
            );

            // Smart value formatting
            let fmt_value = move || {
                let v = value.get();
                if (v - v.round()).abs() < 0.001 {
                    format!("{}", v as i32)
                } else {
                    format!("{:.2}", v)
                }
            };

            view! {
                <div class="group/ctrl rounded-lg px-3 py-2.5 hover:bg-white/[0.02] transition-colors duration-150"
                     title=tooltip.unwrap_or_default()>
                    <div class="flex items-center justify-between mb-2">
                        <label class="text-[11px] text-fg-muted font-medium">{name.clone()}</label>
                        <span class="text-[10px] font-mono tabular-nums px-1.5 py-0.5 rounded"
                              style=badge_style>
                            {fmt_value}
                        </span>
                    </div>
                    <div class="flex items-center gap-2">
                        <input
                            type="range"
                            class="flex-1 cursor-pointer"
                            min=min
                            max=max
                            step=step
                            prop:value=move || value.get()
                            on:input=move |ev| {
                                use wasm_bindgen::JsCast;
                                let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                if let Some(el) = target {
                                    if let Ok(v) = el.value().parse::<f32>() {
                                        set_value.set(v);
                                        on_change.run((control_name.clone(), json!(v)));
                                    }
                                }
                            }
                        />
                    </div>
                </div>
            }.into_any()
        }
        ControlType::Toggle => {
            let initial = matches!(initial_value, ControlValue::Boolean(true));
            let (checked, set_checked) = signal(initial);
            let control_name = control_id.clone();
            let on_style = format!(
                "background: rgba({}, 0.8); box-shadow: 0 0 12px rgba({}, 0.3)",
                accent_rgb, accent_rgb
            );

            view! {
                <div class="group/ctrl rounded-lg px-3 py-2.5 hover:bg-white/[0.02] transition-colors duration-150
                            flex items-center justify-between"
                     title=tooltip.unwrap_or_default()>
                    <label class="text-[11px] text-fg-muted font-medium">{name.clone()}</label>
                    <button
                        class="relative w-10 h-[22px] rounded-full transition-all duration-200"
                        style=move || if checked.get() { on_style.clone() } else { "background: rgba(255,255,255,0.08)".to_string() }
                        on:click=move |_| {
                            let new_val = !checked.get();
                            set_checked.set(new_val);
                            on_change.run((control_name.clone(), json!(new_val)));
                        }
                    >
                        <div
                            class="absolute top-[3px] w-4 h-4 rounded-full shadow-sm transition-all duration-200"
                            class=("translate-x-[22px] bg-white", move || checked.get())
                            class=("translate-x-[3px] bg-fg-dim", move || !checked.get())
                        />
                    </button>
                </div>
            }.into_any()
        }
        ControlType::ColorPicker => {
            let initial = match &initial_value {
                ControlValue::Color([r, g, b, _]) => {
                    format!(
                        "#{:02x}{:02x}{:02x}",
                        (*r * 255.0) as u8,
                        (*g * 255.0) as u8,
                        (*b * 255.0) as u8
                    )
                }
                _ => "#e135ff".to_string(),
            };
            let (color, set_color) = signal(initial);
            let control_name = control_id.clone();

            view! {
                <div class="group/ctrl rounded-lg px-3 py-2.5 hover:bg-white/[0.02] transition-colors duration-150"
                     title=tooltip.unwrap_or_default()>
                    <div class="flex items-center justify-between">
                        <label class="text-[11px] text-fg-muted font-medium">{name.clone()}</label>
                        <span class="text-[10px] font-mono text-fg-dim/60">{move || color.get()}</span>
                    </div>
                    <div class="mt-2 flex items-center gap-3">
                        // Color swatch preview (shows actual color)
                        <div
                            class="w-10 h-10 rounded-lg border border-white/[0.08] shadow-inner"
                            style=move || format!("background: {}; box-shadow: inset 0 1px 2px rgba(0,0,0,0.3), 0 0 12px {}40", color.get(), color.get())
                        />
                        <input
                            type="color"
                            class="flex-1 h-10 rounded-lg border border-white/[0.08] bg-transparent cursor-pointer
                                   hover:border-white/[0.15] transition-colors duration-150"
                            prop:value=move || color.get()
                            on:input=move |ev| {
                                use wasm_bindgen::JsCast;
                                let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                if let Some(el) = target {
                                    let hex = el.value();
                                    set_color.set(hex.clone());
                                    on_change.run((control_name.clone(), json!(hex)));
                                }
                            }
                        />
                    </div>
                </div>
            }.into_any()
        }
        ControlType::Dropdown => {
            let labels = def.labels.clone();
            let initial = match &initial_value {
                ControlValue::Enum(s) => s.clone(),
                _ => labels.first().cloned().unwrap_or_default(),
            };
            let (selected, set_selected) = signal(initial);
            let control_name = control_id.clone();

            view! {
                <div class="group/ctrl rounded-lg px-3 py-2.5 hover:bg-white/[0.02] transition-colors duration-150"
                     title=tooltip.unwrap_or_default()>
                    <label class="text-[11px] text-fg-muted font-medium mb-1.5 block">{name.clone()}</label>
                    <select
                        class="w-full bg-layer-3 border border-white/[0.06] rounded-lg px-3 py-1.5 text-xs text-fg
                               focus:outline-none focus:border-electric-purple/30
                               focus:shadow-[0_0_0_1px_rgba(225,53,255,0.1)]
                               cursor-pointer transition-all duration-150"
                        on:change=move |ev| {
                            use wasm_bindgen::JsCast;
                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
                            if let Some(el) = target {
                                let val = el.value();
                                set_selected.set(val.clone());
                                on_change.run((control_name.clone(), json!(val)));
                            }
                        }
                    >
                        {labels.iter().map(|label| {
                            let label = label.clone();
                            let is_selected = {
                                let label = label.clone();
                                move || selected.get() == label
                            };
                            view! {
                                <option value=label.clone() selected=is_selected>{label.clone()}</option>
                            }
                        }).collect_view()}
                    </select>
                </div>
            }.into_any()
        }
        ControlType::TextInput => {
            let initial = match &initial_value {
                ControlValue::Text(s) => s.clone(),
                _ => String::new(),
            };
            let (text, set_text) = signal(initial);
            let control_name = control_id.clone();

            view! {
                <div class="group/ctrl rounded-lg px-3 py-2.5 hover:bg-white/[0.02] transition-colors duration-150"
                     title=tooltip.unwrap_or_default()>
                    <label class="text-[11px] text-fg-muted font-medium mb-1.5 block">{name.clone()}</label>
                    <input
                        type="text"
                        class="w-full bg-layer-3 border border-white/[0.06] rounded-lg px-3 py-1.5 text-xs text-fg
                               focus:outline-none focus:border-electric-purple/30
                               focus:shadow-[0_0_0_1px_rgba(225,53,255,0.1)]
                               placeholder-fg-dim/40 transition-all duration-150"
                        prop:value=move || text.get()
                        on:change=move |ev| {
                            use wasm_bindgen::JsCast;
                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                            if let Some(el) = target {
                                let val = el.value();
                                set_text.set(val.clone());
                                on_change.run((control_name.clone(), json!(val)));
                            }
                        }
                    />
                </div>
            }.into_any()
        }
        ControlType::GradientEditor => {
            view! {
                <div class="rounded-lg px-3 py-2.5 opacity-40">
                    <label class="text-[11px] text-fg-muted font-medium mb-1 block">{name.clone()}</label>
                    <div class="h-6 rounded-md bg-gradient-to-r from-electric-purple via-neon-cyan to-coral opacity-30" />
                    <span class="text-[9px] text-fg-dim/40 mt-1 block">"Gradient editor coming soon"</span>
                </div>
            }.into_any()
        }
    }
}
