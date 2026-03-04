//! Auto-generated control panel — renders widgets from ControlDefinition metadata.

use leptos::prelude::*;
use serde_json::json;
use std::collections::BTreeMap;

use hypercolor_types::effect::{ControlDefinition, ControlType, ControlValue};

/// Auto-generated control panel for the active effect.
#[component]
pub fn ControlPanel(
    #[prop(into)] controls: Signal<Vec<ControlDefinition>>,
    #[prop(into)] on_change: Callback<(String, serde_json::Value)>,
) -> impl IntoView {
    // Group controls by their group field
    let grouped = Memo::new(move |_| {
        let defs = controls.get();
        let mut groups: BTreeMap<String, Vec<ControlDefinition>> = BTreeMap::new();
        for def in defs {
            let group = def.group.clone().unwrap_or_else(|| "General".to_string());
            groups.entry(group).or_default().push(def);
        }
        groups
    });

    view! {
        <div class="space-y-4">
            {move || grouped.get().into_iter().map(|(group, defs)| {
                view! {
                    <div class="space-y-3">
                        <h4 class="text-[10px] font-mono uppercase tracking-widest text-zinc-600 px-1">
                            {group}
                        </h4>
                        <div class="space-y-2">
                            {defs.into_iter().map(|def| {
                                view! { <ControlWidget def=def on_change=on_change /> }
                            }).collect_view()}
                        </div>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

/// A single control widget, dispatched by ControlType.
#[component]
fn ControlWidget(
    def: ControlDefinition,
    on_change: Callback<(String, serde_json::Value)>,
) -> impl IntoView {
    let name = def.name.clone();
    let control_id = def.control_id().to_owned();
    let tooltip = def.tooltip.clone();

    match def.control_type {
        ControlType::Slider => {
            let initial = def.default_value.as_f32().unwrap_or(0.5);
            let min = def.min.unwrap_or(0.0);
            let max = def.max.unwrap_or(1.0);
            let step = def.step.unwrap_or(0.01);
            let (value, set_value) = signal(initial);
            let control_name = control_id.clone();

            view! {
                <div class="flex items-center gap-3 px-1" title=tooltip.unwrap_or_default()>
                    <label class="text-xs text-zinc-400 w-24 shrink-0 truncate">{name.clone()}</label>
                    <input
                        type="range"
                        class="flex-1 h-1 accent-electric-purple bg-white/5 rounded-full appearance-none cursor-pointer
                               [&::-webkit-slider-thumb]:w-3 [&::-webkit-slider-thumb]:h-3
                               [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-electric-purple
                               [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:shadow-[0_0_6px_rgba(225,53,255,0.4)]"
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
                    <span class="text-[10px] font-mono text-zinc-500 w-10 text-right tabular-nums">
                        {move || format!("{:.2}", value.get())}
                    </span>
                </div>
            }.into_any()
        }
        ControlType::Toggle => {
            let initial = matches!(def.default_value, ControlValue::Boolean(true));
            let (checked, set_checked) = signal(initial);
            let control_name = control_id.clone();

            view! {
                <div class="flex items-center justify-between px-1" title=tooltip.unwrap_or_default()>
                    <label class="text-xs text-zinc-400">{name.clone()}</label>
                    <button
                        class="relative w-9 h-5 rounded-full transition-colors duration-200"
                        class=("bg-electric-purple", move || checked.get())
                        class=("bg-white/10", move || !checked.get())
                        on:click=move |_| {
                            let new_val = !checked.get();
                            set_checked.set(new_val);
                            on_change.run((control_name.clone(), json!(new_val)));
                        }
                    >
                        <div
                            class="absolute top-0.5 w-4 h-4 rounded-full bg-white shadow-sm transition-transform duration-200"
                            class=("translate-x-[18px]", move || checked.get())
                            class=("translate-x-0.5", move || !checked.get())
                        />
                    </button>
                </div>
            }.into_any()
        }
        ControlType::ColorPicker => {
            let initial = match &def.default_value {
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
                <div class="flex items-center gap-3 px-1" title=tooltip.unwrap_or_default()>
                    <label class="text-xs text-zinc-400 w-24 shrink-0 truncate">{name.clone()}</label>
                    <input
                        type="color"
                        class="w-8 h-8 rounded border border-white/10 bg-transparent cursor-pointer"
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
                    <span class="text-[10px] font-mono text-zinc-500">{move || color.get()}</span>
                </div>
            }.into_any()
        }
        ControlType::Dropdown => {
            let labels = def.labels.clone();
            let initial = match &def.default_value {
                ControlValue::Enum(s) => s.clone(),
                _ => labels.first().cloned().unwrap_or_default(),
            };
            let (selected, set_selected) = signal(initial);
            let control_name = control_id.clone();

            view! {
                <div class="flex items-center gap-3 px-1" title=tooltip.unwrap_or_default()>
                    <label class="text-xs text-zinc-400 w-24 shrink-0 truncate">{name.clone()}</label>
                    <select
                        class="flex-1 bg-layer-3 border border-white/5 rounded-md px-2 py-1 text-xs text-zinc-200
                               focus:outline-none focus:border-electric-purple/40 cursor-pointer"
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
            let initial = match &def.default_value {
                ControlValue::Text(s) => s.clone(),
                _ => String::new(),
            };
            let (text, set_text) = signal(initial);
            let control_name = control_id.clone();

            view! {
                <div class="flex items-center gap-3 px-1" title=tooltip.unwrap_or_default()>
                    <label class="text-xs text-zinc-400 w-24 shrink-0 truncate">{name.clone()}</label>
                    <input
                        type="text"
                        class="flex-1 bg-layer-3 border border-white/5 rounded-md px-2 py-1 text-xs text-zinc-200
                               focus:outline-none focus:border-electric-purple/40 placeholder-zinc-600"
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
            // V1 placeholder — gradient editor is complex, defer to later
            view! {
                <div class="flex items-center gap-3 px-1 opacity-50">
                    <label class="text-xs text-zinc-400 w-24 shrink-0 truncate">{name.clone()}</label>
                    <span class="text-[10px] text-zinc-600 italic">"Gradient editor coming soon"</span>
                </div>
            }.into_any()
        }
    }
}
