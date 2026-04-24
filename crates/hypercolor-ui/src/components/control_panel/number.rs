//! Slider renderer — numeric controls (`ControlType::Slider`).

use leptos::prelude::*;
use leptos_icons::Icon;
use serde_json::json;

use hypercolor_leptos_ext::events::Input;
use hypercolor_types::effect::{ControlDefinition, ControlValue};

#[allow(clippy::too_many_arguments)]
pub(super) fn render_slider(
    def: &ControlDefinition,
    name: String,
    control_id: String,
    tooltip: Option<String>,
    icon: icondata::Icon,
    icon_style: String,
    accent_rgb: String,
    value: Signal<ControlValue>,
    on_change: Callback<(String, serde_json::Value)>,
) -> impl IntoView {
    let initial = value.get_untracked().as_f32().unwrap_or(0.5);
    let min = def.min.unwrap_or(0.0);
    let max = def.max.unwrap_or(1.0);
    let step = def.step.unwrap_or(0.01);
    let (slider_value, set_slider_value) = signal(initial);
    let control_name = control_id.clone();

    Effect::new(move |_| {
        let next = value.get().as_f32().unwrap_or(0.5);
        if (slider_value.get_untracked() - next).abs() > f32::EPSILON {
            set_slider_value.set(next);
        }
    });

    let badge_style = format!(
        "color: rgba({}, 0.9); background: rgba({}, 0.08)",
        accent_rgb, accent_rgb
    );

    let fmt_value = move || {
        let v = slider_value.get();
        if (v - v.round()).abs() < 0.001 {
            format!("{}", v as i32)
        } else {
            format!("{:.2}", v)
        }
    };

    view! {
        <div class="flex items-center gap-2.5 rounded-lg px-3 py-2 hover:bg-surface-hover/20 transition-colors duration-200 group"
             title=tooltip.unwrap_or_default()
             style=move || format!("--glow-rgb: {}", accent_rgb)>
            <Icon icon=icon width="15px" height="15px" style=icon_style.clone() />
            <label class="text-xs text-fg-secondary font-medium shrink-0 min-w-[80px] max-w-[120px] truncate">{name.clone()}</label>
            <input
                type="range"
                class="flex-1 min-w-0 cursor-pointer slider-silk"
                min=min
                max=max
                step=step
                prop:value=move || slider_value.get()
                on:input=move |ev| {
                    let event = Input::from_event(ev);
                    if let Some(v) = event.value::<f32>() {
                        set_slider_value.set(v);
                        on_change.run((control_name.clone(), json!(v)));
                    }
                }
            />
            <span class="text-[10px] font-mono tabular-nums w-[36px] text-right shrink-0 px-1.5 py-0.5 rounded"
                  style=badge_style>
                {fmt_value}
            </span>
        </div>
    }
}
