//! Sensor dropdown renderer — proper picker for `ControlKind::Sensor`.
//!
//! Faces declare sensors with `sensor('Label', 'default')`, which surfaces
//! as `ControlKind::Sensor` with `ControlType::TextInput`. Rendering that as
//! a raw text field forced users to memorise sensor IDs. This widget fetches
//! the daemon's current sensor snapshot once per mount and presents a
//! searchable, grouped dropdown with each sensor's live reading — matching
//! the visual language of the enum dropdown.

use leptos::portal::Portal;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;
use serde_json::json;

use hypercolor_types::effect::ControlValue;

use super::{ControlDropdownDismissHandlers, dropdown_panel_style};
use crate::api;

#[derive(Clone, PartialEq)]
struct SensorOption {
    label: String,
    group: &'static str,
    live: String,
}

/// Coarse grouping by label prefix so the dropdown reads in sections.
fn sensor_group(label: &str) -> &'static str {
    let lower = label.to_lowercase();
    if lower.starts_with("cpu") {
        "CPU"
    } else if lower.starts_with("gpu") {
        "GPU"
    } else if lower.starts_with("ram") || lower.starts_with("mem") || lower.starts_with("swap") {
        "Memory"
    } else if lower.starts_with("net") || lower.starts_with("rx") || lower.starts_with("tx") {
        "Network"
    } else if lower.starts_with("fan") || lower.contains("rpm") {
        "Cooling"
    } else if lower.contains("temp") {
        "Thermal"
    } else {
        "Other"
    }
}

fn format_sensor_reading(value: f32, unit: &str) -> String {
    if unit == "°C" || unit == "°F" || unit == "%" {
        format!("{}{unit}", value.round())
    } else if unit == "MB" && value >= 1024.0 {
        format!("{:.1} GB", value / 1024.0)
    } else {
        format!("{} {unit}", value.round())
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_sensor_dropdown(
    name: String,
    control_id: String,
    tooltip: Option<String>,
    icon: icondata::Icon,
    icon_style: String,
    value: Signal<ControlValue>,
    on_change: Callback<(String, serde_json::Value)>,
) -> impl IntoView {
    let initial = match value.get_untracked() {
        ControlValue::Text(s) | ControlValue::Enum(s) => s,
        _ => String::new(),
    };
    let (selected, set_selected) = signal(initial);
    let (dropdown_open, set_dropdown_open) = signal(false);
    let (sensor_options, set_sensor_options) = signal(Vec::<SensorOption>::new());
    let (sensor_search, set_sensor_search) = signal(String::new());
    let trigger_ref = NodeRef::<leptos::html::Button>::new();
    let dropdown_class = format!("control-sensor-{}", control_id);
    let dropdown_wrapper_class = dropdown_class.clone();
    let dropdown_class_value = StoredValue::new(dropdown_class.clone());
    let dropdown_control_name = StoredValue::new(control_id.clone());

    spawn_local(async move {
        if let Ok(snapshot) = api::fetch_system_sensors().await {
            let mut options: Vec<SensorOption> = snapshot
                .readings()
                .into_iter()
                .map(|reading| SensorOption {
                    group: sensor_group(&reading.label),
                    live: format_sensor_reading(reading.value, reading.unit.symbol()),
                    label: reading.label,
                })
                .collect();
            options.sort_by(|a, b| a.group.cmp(b.group).then_with(|| a.label.cmp(&b.label)));
            options.dedup_by(|a, b| a.label == b.label);
            set_sensor_options.set(options);
        }
    });

    Effect::new(move |_| {
        let next = match value.get() {
            ControlValue::Text(s) | ControlValue::Enum(s) => s,
            _ => String::new(),
        };
        if selected.get_untracked() != next {
            set_selected.set(next);
        }
    });

    view! {
        <div
            class="flex items-center gap-2.5 rounded-lg px-3 py-2 hover:bg-surface-hover/20 transition-colors duration-200 group"
            class=("relative", move || dropdown_open.get())
            class=("z-[100]", move || dropdown_open.get())
            title=tooltip.unwrap_or_default()
        >
            <Icon icon=icon width="15px" height="15px" style=icon_style.clone() />
            <label class="text-xs text-fg-secondary font-medium shrink-0 min-w-[80px] max-w-[120px] truncate">
                {name.clone()}
            </label>
            <div class=format!("relative flex-1 min-w-0 {dropdown_wrapper_class}")>
                <button
                    type="button"
                    node_ref=trigger_ref
                    class="w-full flex items-center gap-1.5 bg-surface-sunken border px-2.5 py-1.5
                           text-xs cursor-pointer select-silk-trigger"
                    class=("rounded-t-lg", move || dropdown_open.get())
                    class=("rounded-lg", move || !dropdown_open.get())
                    class=("border-accent-muted", move || dropdown_open.get())
                    class=("border-edge-subtle", move || !dropdown_open.get())
                    on:click=move |_| set_dropdown_open.update(|v| *v = !*v)
                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                        if ev.key() == "Escape" && dropdown_open.get_untracked() {
                            set_dropdown_open.set(false);
                            ev.prevent_default();
                        }
                    }
                >
                    <span class="flex-1 min-w-0 text-left truncate text-fg-primary font-mono">
                        {move || {
                            let v = selected.get();
                            if v.is_empty() { "(unassigned)".to_owned() } else { v }
                        }}
                    </span>
                    <svg
                        class="w-3 h-3 shrink-0 transition-transform duration-200"
                        class=("rotate-180", move || dropdown_open.get())
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                    >
                        <path d="m6 9 6 6 6-6" />
                    </svg>
                </button>

                <Show when=move || dropdown_open.get()>
                    <ControlDropdownDismissHandlers
                        class_name=dropdown_class.clone()
                        is_open=dropdown_open
                        set_open=set_dropdown_open
                    />
                    <Portal>
                        <div class=move || dropdown_class_value.get_value()>
                            <div
                                class="fixed z-[9999]
                                       rounded-b-xl overflow-hidden
                                       bg-surface-overlay/98 backdrop-blur-xl
                                       border border-t-0 border-edge-subtle
                                       dropdown-glow animate-enter-down
                                       overflow-y-auto scrollbar-dropdown"
                                style=move || dropdown_panel_style(trigger_ref.get())
                                on:mousedown=|ev: leptos::ev::MouseEvent| ev.stop_propagation()
                            >
                                <div class="sticky top-0 border-b border-edge-subtle/60 bg-surface-overlay/98 px-2 py-1.5">
                                    <input
                                        type="search"
                                        class="w-full rounded-md border border-edge-subtle bg-surface-sunken px-2 py-1 text-xs text-fg-primary outline-none transition focus:border-accent-primary"
                                        placeholder="Search sensors…"
                                        prop:value=move || sensor_search.get()
                                        on:input=move |event| {
                                            set_sensor_search.set(
                                                hypercolor_leptos_ext::events::Input::from_event(event)
                                                    .value_string()
                                                    .unwrap_or_default(),
                                            )
                                        }
                                        on:click=|ev: leptos::ev::MouseEvent| ev.stop_propagation()
                                    />
                                </div>
                                {move || {
                                    let options = sensor_options.get();
                                    if options.is_empty() {
                                        return view! {
                                            <div class="px-3 py-2 text-xs text-fg-tertiary">
                                                "Loading sensors…"
                                            </div>
                                        }.into_any();
                                    }
                                    let query = sensor_search.get().trim().to_lowercase();
                                    let filtered: Vec<SensorOption> = options
                                        .into_iter()
                                        .filter(|option| {
                                            query.is_empty() || option.label.to_lowercase().contains(&query)
                                        })
                                        .collect();
                                    if filtered.is_empty() {
                                        return view! {
                                            <div class="px-3 py-2 text-xs text-fg-tertiary">
                                                {format!("No sensors match \"{query}\".")}
                                            </div>
                                        }.into_any();
                                    }
                                    let mut rows: Vec<leptos::prelude::AnyView> = Vec::new();
                                    let mut current_group = "";
                                    for option in filtered {
                                        if option.group != current_group {
                                            current_group = option.group;
                                            rows.push(view! {
                                                <div class="px-3 pb-0.5 pt-2 text-[9px] font-semibold uppercase tracking-[0.18em] text-fg-tertiary/70">
                                                    {option.group}
                                                </div>
                                            }.into_any());
                                        }
                                        let label = option.label;
                                        let live = option.live;
                                        let label_val = label.clone();
                                        let m1 = label.clone();
                                        let m2 = label.clone();
                                        let m3_bg = label.clone();
                                        let m3_scale = label.clone();
                                        let m3_opacity = label.clone();
                                        let m4_scale = label.clone();
                                        let m4_opacity = label.clone();
                                        let control_name = dropdown_control_name.get_value();
                                        rows.push(view! {
                                            <button
                                                type="button"
                                                class="dropdown-option w-full text-left px-3 py-[7px] text-xs cursor-pointer
                                                       flex items-center gap-2 font-mono"
                                                class=("dropdown-option-active", move || selected.get() == m1)
                                                class=("text-fg-tertiary", move || selected.get() != m2)
                                                on:click=move |_| {
                                                    set_selected.set(label_val.clone());
                                                    on_change.run((control_name.clone(), json!(label_val.clone())));
                                                    set_dropdown_open.set(false);
                                                }
                                            >
                                                <span
                                                    class="w-1 h-1 rounded-full shrink-0 transition-all duration-200"
                                                    class=("bg-accent-muted", move || selected.get() == m3_bg)
                                                    class=("scale-100", move || selected.get() == m3_scale)
                                                    class=("opacity-100", move || selected.get() == m3_opacity)
                                                    class=("scale-0", move || selected.get() != m4_scale)
                                                    class=("opacity-0", move || selected.get() != m4_opacity)
                                                />
                                                <span class="min-w-0 flex-1 truncate">{label}</span>
                                                <span class="shrink-0 tabular-nums text-[10px] text-fg-tertiary/80">
                                                    {live}
                                                </span>
                                            </button>
                                        }.into_any());
                                    }
                                    rows.collect_view().into_any()
                                }}
                            </div>
                        </div>
                    </Portal>
                </Show>
            </div>
        </div>
    }
}
