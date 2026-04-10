//! Text input renderer — free-form string controls (`ControlType::TextInput`).

use leptos::prelude::*;
use leptos_icons::Icon;
use serde_json::json;

use hypercolor_types::effect::ControlValue;

pub(super) fn render_text_input(
    name: String,
    control_id: String,
    tooltip: Option<String>,
    icon: icondata::Icon,
    icon_style: String,
    value: Signal<ControlValue>,
    on_change: Callback<(String, serde_json::Value)>,
) -> impl IntoView {
    let initial = match value.get_untracked() {
        ControlValue::Enum(s) | ControlValue::Text(s) => s.clone(),
        _ => String::new(),
    };
    let (text, set_text) = signal(initial);
    let control_name = control_id;

    Effect::new(move |_| {
        let next = match value.get() {
            ControlValue::Text(s) | ControlValue::Enum(s) => s,
            _ => String::new(),
        };
        if text.get_untracked() != next {
            set_text.set(next);
        }
    });

    view! {
        <div class="flex items-center gap-2.5 rounded-lg px-3 py-2 hover:bg-surface-hover/20 transition-colors duration-200 group"
             title=tooltip.unwrap_or_default()>
            <Icon icon=icon width="15px" height="15px" style=icon_style.clone() />
            <label class="text-xs text-fg-secondary font-medium shrink-0 min-w-[80px] max-w-[120px] truncate">{name.clone()}</label>
            <input
                type="text"
                class="flex-1 min-w-0 bg-surface-sunken border border-edge-subtle rounded-lg px-2.5 py-1.5 text-xs text-fg-primary
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
    }
}
