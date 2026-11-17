//! Slider renderer — numeric controls (`ControlType::Slider`).

use leptos::prelude::*;
use serde_json::json;

use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::ControlDefinition;

use crate::components::value_slider::{ValueFormat, ValueRange, ValueSlider};

/// The label beside an effect's slider.
const LABEL: &str = "min-w-[80px] max-w-[120px] shrink-0 select-none truncate text-xs font-medium text-fg-secondary";

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
    let min = def.min.unwrap_or(0.0);
    let max = def.max.unwrap_or(1.0);
    // Effects routinely ship sliders with no `step`; a flat 0.01 default
    // turns a 10–120 "Count" into nonsense like 31.84. Derive the step
    // from the range so coarse controls snap to whole numbers.
    let step = def.step.unwrap_or_else(|| default_slider_step(min, max));
    let number = Signal::derive(move || value.get().as_effect_f32().unwrap_or(0.5));
    let on_input = Callback::new(move |value: f32| {
        on_change.run((control_id.clone(), json!(value)));
    });

    view! {
        <ValueSlider
            label=name
            value=number
            range=ValueRange { min, max, step }
            format=ValueFormat::for_step(step)
            default=def.default_value.as_effect_f32()
            on_input=Some(on_input)
            icon=Some((icon, icon_style))
            accent_rgb=Some(accent_rgb)
            tooltip=tooltip
            label_class=LABEL
        />
    }
}

/// Pick a slider step when the control definition omits one. Scales with
/// the range so a wide control (a 10–120 count) snaps to whole numbers
/// while a narrow control (0–1) still resolves finely.
fn default_slider_step(min: f32, max: f32) -> f32 {
    let span = (max - min).abs();
    if span >= 50.0 {
        1.0
    } else if span >= 5.0 {
        0.1
    } else if span >= 0.5 {
        0.01
    } else {
        0.001
    }
}
