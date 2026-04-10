//! Color picker renderer — hex / wheel color controls (`ControlType::ColorPicker`).

use leptos::portal::Portal;
use leptos::prelude::*;
use serde_json::json;

use hypercolor_types::effect::ControlValue;

use crate::components::color_wheel::ColorWheel;

use super::{
    QUICK_COLOR_SWATCHES, color_picker_panel_style, control_value_to_hex, hex_to_rgba,
    install_scroll_close_handler_for_picker, normalize_hex,
};

pub(super) fn render_color_picker(
    name: String,
    control_id: String,
    tooltip: Option<String>,
    value: Signal<ControlValue>,
    on_change: Callback<(String, serde_json::Value)>,
    expanded_picker_id: ReadSignal<Option<String>>,
    set_expanded_picker_id: WriteSignal<Option<String>>,
) -> impl IntoView {
    let initial = control_value_to_hex(&value.get_untracked());
    let (color, set_color) = signal(initial);
    let (hex_input, set_hex_input) = signal(color.get_untracked());
    let picker_id = control_id.clone();
    let is_open = Memo::new({
        let picker_id = picker_id.clone();
        move |_| expanded_picker_id.get().as_deref() == Some(picker_id.as_str())
    });
    let control_name = StoredValue::new(control_id);
    let swatch_ref = NodeRef::<leptos::html::Button>::new();

    Effect::new(move |_| {
        let next = control_value_to_hex(&value.get());
        if color.get_untracked() != next {
            set_color.set(next.clone());
            set_hex_input.set(next);
        }
    });

    // Close on scroll — the popover is portaled with fixed positioning,
    // so scrolling would leave it visually detached from the swatch.
    install_scroll_close_handler_for_picker(set_expanded_picker_id);

    // Wheel callback — receives hex from ColorWheel, propagates to engine
    let on_wheel_change = Callback::new(move |hex: String| {
        if let Some(normalized) = normalize_hex(&hex) {
            set_color.set(normalized.clone());
            set_hex_input.set(normalized.clone());
            if let Some(rgba) = hex_to_rgba(&normalized) {
                on_change.run((control_name.get_value(), json!(rgba)));
            }
        }
    });

    view! {
        <div class="relative rounded-lg px-3 py-2 hover:bg-surface-hover/20 transition-colors duration-200 group"
             title=tooltip.unwrap_or_default()>
            // Trigger row — icon + swatch + label + hex value
            <div class="flex items-center gap-2.5">
                <button
                    type="button"
                    node_ref=swatch_ref
                    class="h-7 w-7 shrink-0 rounded-lg border border-edge-default swatch-glow
                           transition-all duration-200 hover:border-edge-strong hover:scale-105"
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
                <label class="text-xs text-fg-secondary font-medium truncate flex-1 min-w-0">{name.clone()}</label>
                <span class="text-[10px] font-mono text-fg-tertiary/50 uppercase shrink-0">
                    {move || color.get().to_uppercase()}
                </span>
            </div>

            // Popover color picker — portaled to escape overflow clipping
            <Show when=move || is_open.get()>
                <Portal>
                    <div
                        class="fixed z-[9999] rounded-2xl
                               bg-[#0d0b16]/98 backdrop-blur-xl
                               border border-white/[0.06]
                               p-3.5 space-y-2.5
                               color-picker-popover animate-picker-in"
                        style=move || color_picker_panel_style(swatch_ref.get())
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
                                on:input=move |ev| {
                                    use wasm_bindgen::JsCast;
                                    let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                    if let Some(el) = target {
                                        let next = el.value();
                                        set_hex_input.set(next.clone());
                                        if let Some(normalized) = normalize_hex(&next) {
                                            set_color.set(normalized.clone());
                                            if let Some(rgba) = hex_to_rgba(&normalized) {
                                                on_change.run((control_name.get_value(), json!(rgba)));
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
                                            let swatch_hex = swatch_hex.clone();
                                            move |_| {
                                                let normalized = normalize_hex(&swatch_hex).expect("hardcoded swatches are valid");
                                                set_color.set(normalized.clone());
                                                set_hex_input.set(normalized.clone());
                                                if let Some(rgba) = hex_to_rgba(&normalized) {
                                                    on_change.run((control_name.get_value(), json!(rgba)));
                                                }
                                            }
                                        }
                                    />
                                }
                            }).collect_view()}
                        </div>
                    </div>
                </Portal>
            </Show>
        </div>
    }
}
