//! Toggle renderer — boolean controls (`ControlType::Toggle`).

use leptos::prelude::*;
use leptos_icons::Icon;
use serde_json::json;

use hypercolor_types::effect::ControlValue;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_toggle(
    name: String,
    control_id: String,
    tooltip: Option<String>,
    icon: icondata::Icon,
    icon_style: String,
    accent_rgb: String,
    value: Signal<ControlValue>,
    on_change: Callback<(String, serde_json::Value)>,
) -> impl IntoView {
    let initial = matches!(value.get_untracked(), ControlValue::Boolean(true));
    let (checked, set_checked) = signal(initial);
    let control_name = control_id;
    let on_style = format!(
        "background: rgba({}, 0.8); box-shadow: 0 0 12px rgba({}, 0.3)",
        accent_rgb, accent_rgb
    );

    Effect::new(move |_| {
        let next = matches!(value.get(), ControlValue::Boolean(true));
        if checked.get_untracked() != next {
            set_checked.set(next);
        }
    });

    view! {
        <div class="flex items-center gap-2.5 rounded-lg px-3 py-2 hover:bg-surface-hover/20 transition-colors duration-200 group"
             title=tooltip.unwrap_or_default()>
            <Icon icon=icon width="15px" height="15px" style=icon_style.clone() />
            <label class="text-xs text-fg-secondary font-medium shrink-0 min-w-[80px] truncate">{name.clone()}</label>
            <div class="flex-1" />
            <button
                class="relative w-10 h-5 rounded-full toggle-track shrink-0 cursor-pointer"
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
                            "absolute top-[3px] w-3.5 h-3.5 rounded-full toggle-thumb translate-x-[21px] bg-white toggle-thumb-on"
                        } else {
                            "absolute top-[3px] w-3.5 h-3.5 rounded-full toggle-thumb translate-x-[3px] bg-fg-tertiary"
                        }
                    }
                />
            </button>
        </div>
    }
}
