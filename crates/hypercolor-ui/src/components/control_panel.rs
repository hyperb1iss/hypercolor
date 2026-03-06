//! Auto-generated control panel — renders widgets from ControlDefinition metadata.
//! Each control resolves its initial value from live `control_values` (if present),
//! falling back to the definition's `default_value`.

use leptos::prelude::*;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};

use hypercolor_types::effect::{ControlDefinition, ControlType, ControlValue};

const QUICK_COLOR_SWATCHES: [&str; 10] = [
    "#6000fc",
    "#e135ff",
    "#ff6ac1",
    "#80ffea",
    "#f1fa8c",
    "#50fa7b",
    "#82aaff",
    "#ffffff",
    "#ff8c42",
    "#0a0910",
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
                            <div class="text-fg-tertiary/50 text-xs">"No controls available"</div>
                        </div>
                    }.into_any()
                } else {
                    groups.into_iter().map(|(group, items)| {
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
                                    {items.into_iter().enumerate().map(|(i, (def, value, rgb))| {
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
                        style=move || if checked.get() { on_style.clone() } else { "background: rgba(255,255,255,0.08)".to_string() }
                        on:click=move |_| {
                            let new_val = !checked.get();
                            set_checked.set(new_val);
                            on_change.run((control_name.clone(), json!(new_val)));
                        }
                    >
                        <div
                            class="absolute top-[3px] w-4 h-4 rounded-full shadow-sm toggle-thumb"
                            class=("translate-x-[22px] bg-white", move || checked.get())
                            class=("translate-x-[3px] bg-fg-tertiary", move || !checked.get())
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
            let is_expanded = Memo::new({
                let picker_id = picker_id.clone();
                move |_| expanded_picker_id.get().as_deref() == Some(picker_id.as_str())
            });
            let (red, set_red) = signal(255u8);
            let (green, set_green) = signal(255u8);
            let (blue, set_blue) = signal(255u8);
            let control_name = control_id.clone();
            apply_hex_color(
                &color.get_untracked(),
                set_color,
                set_hex_input,
                set_red,
                set_green,
                set_blue,
                None,
            );

            view! {
                <div class="rounded-lg px-3 py-2.5 transition-colors duration-150"
                     title=tooltip.unwrap_or_default()>
                    // Trigger row — swatch + label + hex value
                    <div class="flex items-center gap-3">
                        <button
                            type="button"
                            class="h-9 w-9 shrink-0 rounded-xl border border-edge-default shadow-lg
                                   transition-all duration-200 hover:scale-105 hover:border-edge-strong
                                   active:scale-95"
                            style=move || format!(
                                "background: linear-gradient(145deg, {0}, color-mix(in srgb, {0} 65%, black)); \
                                 box-shadow: 0 0 16px {0}33, inset 0 1px 0 rgba(255,255,255,0.06)",
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

                    // Inline expanded color picker — accordion style, no overlay
                    <Show when=move || is_expanded.get()>
                        <div class="mt-3 space-y-3 rounded-xl bg-surface-sunken border border-edge-subtle
                                    p-3.5 shadow-[0_4px_24px_rgba(0,0,0,0.35)] animate-slide-down">
                            // Header — large preview + hex display + close
                            <div class="flex items-center justify-between">
                                <div class="flex items-center gap-2.5">
                                    <div
                                        class="h-7 w-7 rounded-lg border border-edge-subtle"
                                        style=move || format!(
                                            "background: {}; box-shadow: 0 0 10px {}44",
                                            color.get(), color.get()
                                        )
                                    />
                                    <span class="text-xs font-mono uppercase text-fg-tertiary/70 tracking-wider">
                                        {move || color.get().to_uppercase()}
                                    </span>
                                </div>
                                <button
                                    type="button"
                                    class="px-2.5 py-1 rounded-lg text-[10px] font-medium text-fg-tertiary
                                           border border-edge-subtle bg-surface-overlay/30
                                           hover:bg-surface-hover/50 hover:text-fg-primary hover:border-edge-default
                                           active:scale-95 transition-all duration-150"
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
                                >
                                    "Done"
                                </button>
                            </div>
                            // Hex input
                            <input
                                type="text"
                                class="w-full rounded-lg border border-edge-subtle bg-surface-overlay/40
                                       px-3 py-2 text-sm font-mono uppercase text-fg-primary
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
                                            if normalize_hex(&next).is_some() {
                                                apply_hex_color(
                                                    &next,
                                                    set_color,
                                                    set_hex_input,
                                                    set_red,
                                                    set_green,
                                                    set_blue,
                                                    Some((&on_change, &control_name)),
                                                );
                                            }
                                        }
                                    }
                                }
                                on:blur=move |_| {
                                    let next = hex_input.get();
                                    if normalize_hex(&next).is_some() {
                                        apply_hex_color(
                                            &next,
                                            set_color,
                                            set_hex_input,
                                            set_red,
                                            set_green,
                                            set_blue,
                                            None,
                                        );
                                    } else {
                                        set_hex_input.set(color.get());
                                    }
                                }
                            />

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
                                            class="aspect-square rounded-lg border transition-all duration-150 hover:scale-110"
                                            class=("border-white/30 shadow-[0_0_8px_rgba(225,53,255,0.3)]", move || is_active.get())
                                            class=("border-edge-subtle", move || !is_active.get())
                                            style=format!("background: {swatch_hex}")
                                            on:click={
                                                let control_name = control_name.clone();
                                                let swatch_hex = swatch_hex.clone();
                                                move |_| {
                                                    apply_hex_color(
                                                        &swatch_hex,
                                                        set_color,
                                                        set_hex_input,
                                                        set_red,
                                                        set_green,
                                                        set_blue,
                                                        Some((&on_change, &control_name)),
                                                    );
                                                }
                                            }
                                        />
                                    }
                                }).collect_view()}
                            </div>

                            // RGB channel sliders
                            <div class="space-y-2">
                                <div class="flex items-center gap-2.5">
                                    <span class="text-[10px] font-mono font-medium text-error-red/70 w-2.5">"R"</span>
                                    <input
                                        type="range"
                                        class="flex-1 cursor-pointer color-channel"
                                        min="0"
                                        max="255"
                                        step="1"
                                        prop:value=move || red.get()
                                        style=move || format!(
                                            "background: linear-gradient(90deg, rgb(0,{0},{1}), rgb(255,{0},{1}))",
                                            green.get(),
                                            blue.get()
                                        )
                                        on:input={
                                            let control_name = control_name.clone();
                                            move |ev| {
                                                use wasm_bindgen::JsCast;
                                                let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                                if let Some(el) = target
                                                    && let Ok(v) = el.value().parse::<u8>()
                                                {
                                                    apply_rgb_color(
                                                        v,
                                                        green.get(),
                                                        blue.get(),
                                                        set_color,
                                                        set_hex_input,
                                                        set_red,
                                                        set_green,
                                                        set_blue,
                                                        Some((&on_change, &control_name)),
                                                    );
                                                }
                                            }
                                        }
                                    />
                                    <span class="text-[10px] font-mono text-fg-tertiary/60 w-7 text-right tabular-nums">
                                        {move || red.get()}
                                    </span>
                                </div>
                                <div class="flex items-center gap-2.5">
                                    <span class="text-[10px] font-mono font-medium text-success-green/70 w-2.5">"G"</span>
                                    <input
                                        type="range"
                                        class="flex-1 cursor-pointer color-channel"
                                        min="0"
                                        max="255"
                                        step="1"
                                        prop:value=move || green.get()
                                        style=move || format!(
                                            "background: linear-gradient(90deg, rgb({0},0,{1}), rgb({0},255,{1}))",
                                            red.get(),
                                            blue.get()
                                        )
                                        on:input={
                                            let control_name = control_name.clone();
                                            move |ev| {
                                                use wasm_bindgen::JsCast;
                                                let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                                if let Some(el) = target
                                                    && let Ok(v) = el.value().parse::<u8>()
                                                {
                                                    apply_rgb_color(
                                                        red.get(),
                                                        v,
                                                        blue.get(),
                                                        set_color,
                                                        set_hex_input,
                                                        set_red,
                                                        set_green,
                                                        set_blue,
                                                        Some((&on_change, &control_name)),
                                                    );
                                                }
                                            }
                                        }
                                    />
                                    <span class="text-[10px] font-mono text-fg-tertiary/60 w-7 text-right tabular-nums">
                                        {move || green.get()}
                                    </span>
                                </div>
                                <div class="flex items-center gap-2.5">
                                    <span class="text-[10px] font-mono font-medium text-info-blue/70 w-2.5">"B"</span>
                                    <input
                                        type="range"
                                        class="flex-1 cursor-pointer color-channel"
                                        min="0"
                                        max="255"
                                        step="1"
                                        prop:value=move || blue.get()
                                        style=move || format!(
                                            "background: linear-gradient(90deg, rgb({0},{1},0), rgb({0},{1},255))",
                                            red.get(),
                                            green.get()
                                        )
                                        on:input={
                                            let control_name = control_name.clone();
                                            move |ev| {
                                                use wasm_bindgen::JsCast;
                                                let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                                if let Some(el) = target
                                                    && let Ok(v) = el.value().parse::<u8>()
                                                {
                                                    apply_rgb_color(
                                                        red.get(),
                                                        green.get(),
                                                        v,
                                                        set_color,
                                                        set_hex_input,
                                                        set_red,
                                                        set_green,
                                                        set_blue,
                                                        Some((&on_change, &control_name)),
                                                    );
                                                }
                                            }
                                        }
                                    />
                                    <span class="text-[10px] font-mono text-fg-tertiary/60 w-7 text-right tabular-nums">
                                        {move || blue.get()}
                                    </span>
                                </div>
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
                               focus:shadow-[0_0_0_1px_rgba(225,53,255,0.1)]
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
                               focus:shadow-[0_0_0_1px_rgba(225,53,255,0.1)]
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
        ControlValue::Color([r, g, b, _]) => format!(
            "#{:02x}{:02x}{:02x}",
            to_byte(*r),
            to_byte(*g),
            to_byte(*b)
        ),
        ControlValue::Text(hex) if hex.starts_with('#') && hex.len() >= 7 => hex[..7].to_string(),
        _ => "#ffffff".to_string(),
    }
}

fn apply_hex_color(
    raw_hex: &str,
    set_color: WriteSignal<String>,
    set_hex_input: WriteSignal<String>,
    set_red: WriteSignal<u8>,
    set_green: WriteSignal<u8>,
    set_blue: WriteSignal<u8>,
    on_change: Option<(&Callback<(String, serde_json::Value)>, &str)>,
) {
    let Some(normalized) = normalize_hex(raw_hex) else {
        return;
    };
    let Some(rgba) = hex_to_rgba(&normalized) else {
        return;
    };

    set_color.set(normalized.clone());
    set_hex_input.set(normalized.clone());
    set_red.set(to_byte(rgba[0]));
    set_green.set(to_byte(rgba[1]));
    set_blue.set(to_byte(rgba[2]));

    if let Some((callback, control_name)) = on_change {
        callback.run((control_name.to_owned(), json!(rgba)));
    }
}

#[expect(clippy::too_many_arguments)]
fn apply_rgb_color(
    red: u8,
    green: u8,
    blue: u8,
    set_color: WriteSignal<String>,
    set_hex_input: WriteSignal<String>,
    set_red: WriteSignal<u8>,
    set_green: WriteSignal<u8>,
    set_blue: WriteSignal<u8>,
    on_change: Option<(&Callback<(String, serde_json::Value)>, &str)>,
) {
    let hex = format!("#{red:02x}{green:02x}{blue:02x}");
    apply_hex_color(
        &hex,
        set_color,
        set_hex_input,
        set_red,
        set_green,
        set_blue,
        on_change,
    );
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
        f32::from(red) / 255.0,
        f32::from(green) / 255.0,
        f32::from(blue) / 255.0,
        1.0,
    ])
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn to_byte(channel: f32) -> u8 {
    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
}
