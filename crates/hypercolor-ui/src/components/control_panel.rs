//! Auto-generated control panel — renders widgets from ControlDefinition metadata.
//! Each control resolves its initial value from live `control_values` (if present),
//! falling back to the definition's `default_value`.

use leptos::prelude::*;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};

use hypercolor_types::canvas::{linear_to_srgb, srgb_to_linear};
use hypercolor_types::effect::{ControlDefinition, ControlType, ControlValue};

use super::color_wheel::ColorWheel;

const QUICK_COLOR_SWATCHES: [&str; 10] = [
    "#6000fc", "#e135ff", "#ff6ac1", "#80ffea", "#f1fa8c", "#50fa7b", "#82aaff", "#ffffff",
    "#ff8c42", "#0a0910",
];

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
    // Lifted state: which color picker is currently expanded (survives inner re-renders)
    let (expanded_picker_id, set_expanded_picker_id) = signal(Option::<String>::None);

    // Group by definition structure only — NOT by control_values.
    // This prevents the entire widget tree from being torn down on every value change.
    let grouped = Memo::new(move |_| {
        let defs = controls.get();
        let rgb = accent_rgb.get();
        let mut groups: BTreeMap<String, Vec<(ControlDefinition, String)>> = BTreeMap::new();
        for def in defs {
            let group = def.group.clone().unwrap_or_else(|| "General".to_string());
            groups.entry(group).or_default().push((def, rgb.clone()));
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
                            <div class="text-fg-tertiary/50 text-xs">"No controls available"</div>
                        </div>
                    }.into_any()
                } else {
                    // Snapshot current values for initial widget state (untracked — no dependency)
                    let values = control_values.get_untracked();
                    groups.into_iter().map(|(group, items)| {
                        let values = values.clone();
                        view! {
                            <div class="space-y-3 animate-fade-in-up">
                                <div class="flex items-center gap-2">
                                    <div class="h-px flex-1 bg-border-subtle" />
                                    <h4 class="text-[9px] font-mono uppercase tracking-[0.2em] text-fg-tertiary/60 shrink-0">
                                        {group}
                                    </h4>
                                    <div class="h-px flex-1 bg-border-subtle" />
                                </div>
                                <div class="space-y-1">
                                    {items.into_iter().enumerate().map(|(i, (def, rgb))| {
                                        let value = effective_value(&def, &values);
                                        let delay = format!("animation-delay: {}ms", i * 40);
                                        view! {
                                            <div class="animate-fade-in-up" style=delay>
                                                <ControlWidget def=def initial_value=value accent_rgb=rgb on_change=on_change expanded_picker_id=expanded_picker_id set_expanded_picker_id=set_expanded_picker_id />
                                            </div>
                                        }
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
    expanded_picker_id: ReadSignal<Option<String>>,
    set_expanded_picker_id: WriteSignal<Option<String>>,
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
                <div class="group/ctrl rounded-lg px-3 py-2.5 hover:bg-surface-hover/20 transition-colors duration-150"
                     title=tooltip.unwrap_or_default()>
                    <div class="flex items-center justify-between mb-2">
                        <label class="text-xs text-fg-secondary font-medium">{name.clone()}</label>
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
                <div class="group/ctrl rounded-lg px-3 py-2.5 hover:bg-surface-hover/20 transition-colors duration-150
                            flex items-center justify-between"
                     title=tooltip.unwrap_or_default()>
                    <label class="text-xs text-fg-secondary font-medium">{name.clone()}</label>
                    <button
                        class="relative w-10 h-[22px] rounded-full toggle-track"
                        class=("toggle-track-on", move || checked.get())
                        style=move || if checked.get() { on_style.clone() } else { "background: rgba(255,255,255,0.08)".to_string() }
                        on:click=move |_| {
                            let new_val = !checked.get();
                            set_checked.set(new_val);
                            on_change.run((control_name.clone(), json!(new_val)));
                        }
                    >
                        <div
                            class=move || {
                                if checked.get() {
                                    "absolute top-[3px] w-4 h-4 rounded-full toggle-thumb translate-x-[22px] bg-white toggle-thumb-on"
                                } else {
                                    "absolute top-[3px] w-4 h-4 rounded-full toggle-thumb translate-x-[3px] bg-fg-tertiary"
                                }
                            }
                        />
                    </button>
                </div>
            }.into_any()
        }
        ControlType::ColorPicker => {
            let initial = control_value_to_hex(&initial_value);
            let (color, set_color) = signal(initial);
            let (hex_input, set_hex_input) = signal(color.get_untracked());
            let picker_id = control_id.clone();
            let is_open = Memo::new({
                let picker_id = picker_id.clone();
                move |_| expanded_picker_id.get().as_deref() == Some(picker_id.as_str())
            });
            let control_name = control_id.clone();

            // Wheel callback — receives hex from ColorWheel, propagates to engine
            let on_wheel_change = Callback::new({
                let control_name = control_name.clone();
                move |hex: String| {
                    if let Some(normalized) = normalize_hex(&hex) {
                        set_color.set(normalized.clone());
                        set_hex_input.set(normalized.clone());
                        if let Some(rgba) = hex_to_rgba(&normalized) {
                            on_change.run((control_name.clone(), json!(rgba)));
                        }
                    }
                }
            });

            view! {
                <div class="relative rounded-lg px-3 py-2.5 transition-colors duration-150"
                     title=tooltip.unwrap_or_default()>
                    // Trigger row — swatch + label + hex value
                    <div class="flex items-center gap-3">
                        <button
                            type="button"
                            class="h-9 w-9 shrink-0 rounded-xl border border-edge-default swatch-glow
                                   transition-all duration-200 hover:border-edge-strong"
                            style=move || format!(
                                "background: linear-gradient(145deg, {0}, color-mix(in srgb, {0} 65%, black)); \
                                 --swatch-color: {0}55",
                                color.get()
                            )
                            on:click={
                                let picker_id = picker_id.clone();
                                move |_| {
                                    set_expanded_picker_id.update(|current| {
                                        if current.as_deref() == Some(picker_id.as_str()) {
                                            *current = None;
                                        } else {
                                            *current = Some(picker_id.clone());
                                        }
                                    });
                                }
                            }
                        />
                        <div class="min-w-0 flex-1">
                            <div class="flex items-center justify-between">
                                <label class="text-xs text-fg-secondary font-medium">{name.clone()}</label>
                                <span class="text-[10px] font-mono text-fg-tertiary/60 uppercase tracking-wider">
                                    {move || color.get().to_uppercase()}
                                </span>
                            </div>
                        </div>
                    </div>

                    // Popover color picker — overlays above the control
                    <Show when=move || is_open.get()>
                        // Backdrop scrim — click anywhere outside to close
                        <div
                            class="fixed inset-0 z-40 bg-black/20"
                            on:click={
                                let picker_id = picker_id.clone();
                                move |_| {
                                    set_expanded_picker_id.update(|current| {
                                        if current.as_deref() == Some(picker_id.as_str()) {
                                            *current = None;
                                        }
                                    });
                                }
                            }
                        />
                        // Popover panel
                        <div class="absolute left-1/2 -translate-x-1/2 bottom-full mb-2 z-50
                                    w-[260px] rounded-2xl
                                    bg-surface-sunken/95 backdrop-blur-xl
                                    border border-edge-subtle
                                    p-4 space-y-3
                                    color-picker-popover animate-scale-in">

                            // Color wheel canvas
                            <div class="flex justify-center">
                                <ColorWheel
                                    color=Signal::derive(move || color.get())
                                    on_change=on_wheel_change
                                />
                            </div>

                            // Hex input + preview swatch
                            <div class="flex items-center gap-2">
                                <div
                                    class="h-8 w-8 shrink-0 rounded-lg border border-edge-subtle"
                                    style=move || format!(
                                        "background: {}; box-shadow: 0 0 14px {}44",
                                        color.get(), color.get()
                                    )
                                />
                                <input
                                    type="text"
                                    class="flex-1 rounded-lg border border-edge-subtle bg-surface-overlay/40
                                           px-2.5 py-1.5 text-xs font-mono uppercase text-fg-primary
                                           placeholder-fg-tertiary/30 focus:outline-none
                                           focus:border-accent-muted transition-all duration-150"
                                    maxlength="7"
                                    placeholder="#E135FF"
                                    prop:value=move || hex_input.get()
                                    on:input={
                                        let control_name = control_name.clone();
                                        move |ev| {
                                            use wasm_bindgen::JsCast;
                                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                            if let Some(el) = target {
                                                let next = el.value();
                                                set_hex_input.set(next.clone());
                                                if let Some(normalized) = normalize_hex(&next) {
                                                    set_color.set(normalized.clone());
                                                    if let Some(rgba) = hex_to_rgba(&normalized) {
                                                        on_change.run((control_name.clone(), json!(rgba)));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    on:blur=move |_| {
                                        let next = hex_input.get();
                                        if normalize_hex(&next).is_some() {
                                            set_hex_input.set(normalize_hex(&next).expect("validated"));
                                        } else {
                                            set_hex_input.set(color.get());
                                        }
                                    }
                                />
                            </div>

                            // Quick pick swatches
                            <div class="grid grid-cols-10 gap-1.5">
                                {QUICK_COLOR_SWATCHES.into_iter().map(|swatch| {
                                    let swatch_hex = swatch.to_string();
                                    let is_active = {
                                        let swatch_hex = swatch_hex.clone();
                                        Memo::new(move |_| color.get() == swatch_hex)
                                    };
                                    view! {
                                        <button
                                            type="button"
                                            class=move || {
                                                if is_active.get() {
                                                    "aspect-square rounded-lg border transition-all duration-150 hover:scale-110 border-white/30 edge-glow-accent"
                                                } else {
                                                    "aspect-square rounded-lg border transition-all duration-150 hover:scale-110 border-edge-subtle"
                                                }
                                            }
                                            style=format!("background: {swatch_hex}")
                                            on:click={
                                                let control_name = control_name.clone();
                                                let swatch_hex = swatch_hex.clone();
                                                move |_| {
                                                    let normalized = normalize_hex(&swatch_hex).expect("hardcoded swatches are valid");
                                                    set_color.set(normalized.clone());
                                                    set_hex_input.set(normalized.clone());
                                                    if let Some(rgba) = hex_to_rgba(&normalized) {
                                                        on_change.run((control_name.clone(), json!(rgba)));
                                                    }
                                                }
                                            }
                                        />
                                    }
                                }).collect_view()}
                            </div>
                        </div>
                    </Show>
                </div>
            }.into_any()
        }
        ControlType::Dropdown => {
            let labels = def.labels.clone();
            let initial = match &initial_value {
                ControlValue::Enum(s) | ControlValue::Text(s) => s.clone(),
                _ => labels.first().cloned().unwrap_or_default(),
            };
            let (selected, set_selected) = signal(initial);
            let control_name = control_id.clone();

            view! {
                <div class="group/ctrl rounded-lg px-3 py-2.5 hover:bg-surface-hover/20 transition-colors duration-150"
                     title=tooltip.unwrap_or_default()>
                    <label class="text-xs text-fg-primary-muted font-medium mb-1.5 block">{name.clone()}</label>
                    <select
                        class="w-full bg-surface-sunken border border-edge-subtle rounded-lg px-3 py-1.5 text-xs text-fg-primary
                               focus:outline-none focus:border-accent-muted
                               focus:border-accent-muted glow-ring
                               cursor-pointer transition-all duration-150"
                        prop:value=move || selected.get()
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
                            view! {
                                <option value=label.clone()>{label.clone()}</option>
                            }
                        }).collect_view()}
                    </select>
                </div>
            }.into_any()
        }
        ControlType::TextInput => {
            let initial = match &initial_value {
                ControlValue::Text(s) | ControlValue::Enum(s) => s.clone(),
                _ => String::new(),
            };
            let (text, set_text) = signal(initial);
            let control_name = control_id.clone();

            view! {
                <div class="group/ctrl rounded-lg px-3 py-2.5 hover:bg-surface-hover/20 transition-colors duration-150"
                     title=tooltip.unwrap_or_default()>
                    <label class="text-xs text-fg-primary-muted font-medium mb-1.5 block">{name.clone()}</label>
                    <input
                        type="text"
                        class="w-full bg-surface-sunken border border-edge-subtle rounded-lg px-3 py-1.5 text-xs text-fg-primary
                               focus:outline-none focus:border-accent-muted
                               focus:border-accent-muted glow-ring
                               placeholder-fg-tertiary/40 transition-all duration-150"
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
                    <label class="text-xs text-fg-primary-muted font-medium mb-1 block">{name.clone()}</label>
                    <div class="h-6 rounded-md bg-gradient-to-r from-electric-purple via-neon-cyan to-coral opacity-30" />
                    <span class="text-[9px] text-fg-primary-dim/40 mt-1 block">"Gradient editor coming soon"</span>
                </div>
            }.into_any()
        }
    }
}

fn control_value_to_hex(value: &ControlValue) -> String {
    match value {
        ControlValue::Color([r, g, b, _]) => {
            format!("#{:02x}{:02x}{:02x}", to_byte(*r), to_byte(*g), to_byte(*b))
        }
        ControlValue::Text(hex) if hex.starts_with('#') && hex.len() >= 7 => hex[..7].to_string(),
        _ => "#ffffff".to_string(),
    }
}

fn normalize_hex(raw_hex: &str) -> Option<String> {
    let trimmed = raw_hex.trim();
    let trimmed = trimmed.strip_prefix('#').unwrap_or(trimmed);
    let expanded = match trimmed.len() {
        3 => trimmed
            .chars()
            .flat_map(|ch| [ch, ch])
            .collect::<String>()
            .to_ascii_lowercase(),
        6 => trimmed.to_ascii_lowercase(),
        _ => return None,
    };

    if expanded.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(format!("#{expanded}"))
    } else {
        None
    }
}

fn hex_to_rgba(hex: &str) -> Option<[f32; 4]> {
    let normalized = normalize_hex(hex)?;
    let compact = normalized.strip_prefix('#').unwrap_or(normalized.as_str());
    let red = u8::from_str_radix(&compact[0..2], 16).ok()?;
    let green = u8::from_str_radix(&compact[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&compact[4..6], 16).ok()?;

    Some([
        srgb_to_linear(f32::from(red) / 255.0),
        srgb_to_linear(f32::from(green) / 255.0),
        srgb_to_linear(f32::from(blue) / 255.0),
        1.0,
    ])
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn to_byte(channel: f32) -> u8 {
    (linear_to_srgb(channel.clamp(0.0, 1.0)) * 255.0).round() as u8
}
