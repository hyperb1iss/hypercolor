//! Auto-generated control panel — renders widgets from ControlDefinition metadata.
//! Each control resolves its initial value from live `control_values` (if present),
//! falling back to the definition's `default_value`.

use leptos::prelude::*;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use wasm_bindgen::prelude::*;

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

    // Global click-outside handler — closes any open color picker when clicking
    // outside its popover. Uses document-level mousedown so it works regardless
    // of sidebar stacking contexts / overflow clipping.
    install_click_outside_handler(set_expanded_picker_id);

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
        <div class="space-y-0.5">
            {move || {
                let groups = grouped.get();
                if groups.is_empty() {
                    view! {
                        <div class="text-center py-4">
                            <div class="text-fg-tertiary/50 text-[11px]">"No controls available"</div>
                        </div>
                    }.into_any()
                } else {
                    let values = control_values.get_untracked();
                    let total_groups = groups.len();
                    groups.into_iter().map(|(group, items)| {
                        let values = values.clone();
                        // Only show group header for non-General groups when there are multiple groups
                        let show_header = total_groups > 1 && group != "General";
                        view! {
                            <div class="animate-fade-in-up">
                                {show_header.then(|| view! {
                                    <div class="flex items-center gap-2 mt-2 mb-1">
                                        <div class="h-px flex-1 bg-border-subtle" />
                                        <h4 class="text-[8px] font-mono uppercase tracking-[0.2em] text-fg-tertiary/50 shrink-0">
                                            {group.clone()}
                                        </h4>
                                        <div class="h-px flex-1 bg-border-subtle" />
                                    </div>
                                })}
                                {items.into_iter().enumerate().map(|(i, (def, rgb))| {
                                    let value = effective_value(&def, &values);
                                    let delay = format!("animation-delay: {}ms", i * 30);
                                    view! {
                                        <div class="animate-fade-in-up" style=delay>
                                            <ControlWidget def=def initial_value=value accent_rgb=rgb on_change=on_change expanded_picker_id=expanded_picker_id set_expanded_picker_id=set_expanded_picker_id />
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        }
                    }).collect_view().into_any()
                }
            }}
        </div>
    }
}

/// Install a one-time document-level mousedown listener that closes the color
/// picker when clicking outside `.color-picker-popover` or `.swatch-glow`.
/// ControlPanel is effectively a singleton so the leak from `forget()` is fine.
fn install_click_outside_handler(set_expanded: WriteSignal<Option<String>>) {
    let handler = Closure::<dyn Fn(web_sys::Event)>::new(move |ev: web_sys::Event| {
        let inside = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .map(|el| {
                el.closest(".color-picker-popover")
                    .ok()
                    .flatten()
                    .is_some()
                    || el.closest(".swatch-glow").ok().flatten().is_some()
            })
            .unwrap_or(false);

        if !inside {
            set_expanded.set(None);
        }
    });

    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        let _ = doc
            .add_event_listener_with_callback("mousedown", handler.as_ref().unchecked_ref());
    }
    handler.forget();
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

            let badge_style = format!(
                "color: rgba({}, 0.9); background: rgba({}, 0.08)",
                accent_rgb, accent_rgb
            );

            let fmt_value = move || {
                let v = value.get();
                if (v - v.round()).abs() < 0.001 {
                    format!("{}", v as i32)
                } else {
                    format!("{:.2}", v)
                }
            };

            view! {
                <div class="flex items-center gap-2 rounded px-2.5 py-1.5 hover:bg-surface-hover/20 transition-colors duration-150"
                     title=tooltip.unwrap_or_default()>
                    <label class="text-[11px] text-fg-secondary font-medium shrink-0 w-[100px] truncate">{name.clone()}</label>
                    <input
                        type="range"
                        class="flex-1 min-w-0 cursor-pointer"
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
                    <span class="text-[10px] font-mono tabular-nums w-[32px] text-right shrink-0 px-1 rounded"
                          style=badge_style>
                        {fmt_value}
                    </span>
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
                <div class="flex items-center gap-2 rounded px-2.5 py-1.5 hover:bg-surface-hover/20 transition-colors duration-150"
                     title=tooltip.unwrap_or_default()>
                    <label class="text-[11px] text-fg-secondary font-medium shrink-0 w-[100px] truncate">{name.clone()}</label>
                    <div class="flex-1" />
                    <button
                        class="relative w-9 h-[18px] rounded-full toggle-track shrink-0"
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
                                    "absolute top-[2px] w-3.5 h-3.5 rounded-full toggle-thumb translate-x-[19px] bg-white toggle-thumb-on"
                                } else {
                                    "absolute top-[2px] w-3.5 h-3.5 rounded-full toggle-thumb translate-x-[2px] bg-fg-tertiary"
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
                <div class="relative rounded px-2 py-1 transition-colors duration-150"
                     title=tooltip.unwrap_or_default()>
                    // Trigger row — swatch + label + hex value
                    <div class="flex items-center gap-2">
                        <button
                            type="button"
                            class="h-6 w-6 shrink-0 rounded-lg border border-edge-default swatch-glow
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
                        <label class="text-[11px] text-fg-secondary font-medium truncate flex-1 min-w-0">{name.clone()}</label>
                        <span class="text-[9px] font-mono text-fg-tertiary/60 uppercase shrink-0">
                            {move || color.get().to_uppercase()}
                        </span>
                    </div>

                    // Popover color picker — floats above the control
                    <Show when=move || is_open.get()>
                        <div
                            class="absolute left-1/2 -translate-x-1/2 bottom-full mb-2 z-50
                                   w-[252px] rounded-2xl
                                   bg-[#0d0b16]/98 backdrop-blur-xl
                                   border border-white/[0.06]
                                   p-3.5 space-y-2.5
                                   color-picker-popover animate-picker-in"
                            on:mousedown=|ev: leptos::ev::MouseEvent| ev.stop_propagation()
                        >
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
                                    class="h-7 w-7 shrink-0 rounded-lg border border-white/[0.08]"
                                    style=move || format!(
                                        "background: {}; box-shadow: 0 0 12px {}55",
                                        color.get(), color.get()
                                    )
                                />
                                <input
                                    type="text"
                                    class="flex-1 min-w-0 rounded-lg border border-white/[0.06] bg-white/[0.04]
                                           px-2.5 py-1.5 text-xs font-mono uppercase text-fg-primary
                                           placeholder-fg-tertiary/30 focus:outline-none
                                           focus:border-accent-muted/50 transition-all duration-150"
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
                                        if let Some(n) = normalize_hex(&next) {
                                            set_hex_input.set(n);
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
                <div class="flex items-center gap-2 rounded px-2.5 py-1.5 hover:bg-surface-hover/20 transition-colors duration-150"
                     title=tooltip.unwrap_or_default()>
                    <label class="text-[11px] text-fg-secondary font-medium shrink-0 w-[100px] truncate">{name.clone()}</label>
                    <select
                        class="flex-1 min-w-0 bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1 text-xs text-fg-primary
                               focus:outline-none focus:border-accent-muted glow-ring
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
                <div class="flex items-center gap-2 rounded px-2.5 py-1.5 hover:bg-surface-hover/20 transition-colors duration-150"
                     title=tooltip.unwrap_or_default()>
                    <label class="text-[11px] text-fg-secondary font-medium shrink-0 w-[100px] truncate">{name.clone()}</label>
                    <input
                        type="text"
                        class="flex-1 min-w-0 bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1 text-xs text-fg-primary
                               focus:outline-none focus:border-accent-muted glow-ring
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
                <div class="flex items-center gap-2 rounded px-2 py-1 opacity-40">
                    <label class="text-[11px] text-fg-secondary font-medium shrink-0 w-[100px] truncate">{name.clone()}</label>
                    <div class="flex-1 h-4 rounded bg-gradient-to-r from-electric-purple via-neon-cyan to-coral opacity-30" />
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
