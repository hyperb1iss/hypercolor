use leptos::prelude::*;

use hypercolor_leptos_ext::events::document as browser_document;
use hypercolor_types::effect::ControlValue;

use crate::control_value_json::hex_to_rgba_json;

#[derive(Clone, Copy)]
struct PoisonousThemePalette {
    bg: &'static str,
    colors: [&'static str; 3],
}

fn poisonous_theme_palette(theme: &str) -> Option<PoisonousThemePalette> {
    match theme {
        "Poison" => Some(PoisonousThemePalette {
            bg: "#130032",
            colors: ["#6000fc", "#b300ff", "#8a42ff"],
        }),
        "Blacklight" => Some(PoisonousThemePalette {
            bg: "#06050d",
            colors: ["#ff58c8", "#30e5ff", "#f4f24e"],
        }),
        "Radioactive" => Some(PoisonousThemePalette {
            bg: "#060b05",
            colors: ["#7bff00", "#00ff9d", "#f3ff52"],
        }),
        "Nightshade" => Some(PoisonousThemePalette {
            bg: "#0b0615",
            colors: ["#8d5cff", "#ff4fd1", "#56d8ff"],
        }),
        "Cotton Candy" => Some(PoisonousThemePalette {
            bg: "#110816",
            colors: ["#ff74c5", "#79ecff", "#ffe869"],
        }),
        _ => None,
    }
}

fn is_poisonous_color_control(control_name: &str) -> bool {
    matches!(control_name, "bgColor" | "color1" | "color2" | "color3")
}

fn control_text_value(value: &ControlValue) -> Option<&str> {
    match value {
        ControlValue::Text(text) | ControlValue::Enum(text) => Some(text.as_str()),
        _ => None,
    }
}

pub(super) fn expand_control_updates(
    active_effect_name: Option<&str>,
    current_values: &std::collections::HashMap<String, ControlValue>,
    control_name: &str,
    value: &serde_json::Value,
) -> Vec<(String, serde_json::Value)> {
    let mut updates = vec![(control_name.to_owned(), value.clone())];

    if active_effect_name != Some("Poisonous") {
        return updates;
    }

    if control_name == "theme"
        && let Some(theme_name) = value.as_str()
        && let Some(palette) = poisonous_theme_palette(theme_name)
    {
        for (name, hex) in [
            ("bgColor", palette.bg),
            ("color1", palette.colors[0]),
            ("color2", palette.colors[1]),
            ("color3", palette.colors[2]),
        ] {
            if let Some(color_value) = hex_to_rgba_json(hex) {
                updates.push((name.to_owned(), color_value));
            }
        }
    }

    if is_poisonous_color_control(control_name) {
        let active_theme = current_values
            .get("theme")
            .and_then(control_text_value)
            .unwrap_or("Poison");
        if active_theme != "Custom" {
            updates.push(("theme".to_owned(), serde_json::json!("Custom")));
        }
    }

    updates
}

fn toggle_body_resizing(active: bool) {
    if let Some(body) = browser_document().and_then(|d| d.body()) {
        if active {
            let _ = body.class_list().add_1("resizing");
        } else {
            let _ = body.class_list().remove_1("resizing");
        }
    }
}

/// Build the three drag callbacks (start, move, end) for a resizable panel.
pub(super) fn drag_callbacks(
    width: ReadSignal<f64>,
    set_width: WriteSignal<f64>,
    min: f64,
    max: f64,
    storage_key: &'static str,
) -> (Callback<()>, Callback<f64>, Callback<()>) {
    let drag_start = StoredValue::new(0.0_f64);

    let on_start = Callback::new(move |()| {
        drag_start.set_value(width.get_untracked());
        toggle_body_resizing(true);
    });
    let on_drag = Callback::new(move |delta_x: f64| {
        let new_w = (drag_start.get_value() - delta_x).clamp(min, max);
        set_width.set(new_w);
    });
    let on_end = Callback::new(move |()| {
        toggle_body_resizing(false);
        persist_to_storage(storage_key, &width.get_untracked().to_string());
    });

    (on_start, on_drag, on_end)
}

pub(super) fn persist_to_storage(key: &str, value: &str) {
    crate::storage::set(key, value);
}

/// Loading skeleton for the effects grid.
#[component]
pub(super) fn LoadingSkeleton() -> impl IntoView {
    view! {
        <div class="grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] gap-4">
            {(0..12).map(|_| {
                view! {
                    <div class="rounded-xl border border-edge-subtle bg-surface-overlay/40 px-4 py-3 animate-pulse space-y-3">
                        <div class="flex justify-between">
                            <div class="h-4 w-28 bg-surface-overlay/40 rounded" />
                            <div class="h-4 w-14 bg-surface-overlay/40 rounded-full" />
                        </div>
                        <div class="space-y-1.5">
                            <div class="h-3 w-full bg-surface-overlay/20 rounded" />
                            <div class="h-3 w-3/4 bg-surface-overlay/20 rounded" />
                        </div>
                        <div class="flex gap-1.5">
                            <div class="h-4 w-14 bg-surface-overlay/20 rounded" />
                            <div class="h-4 w-12 bg-surface-overlay/20 rounded" />
                        </div>
                        <div class="flex justify-between pt-1 border-t border-edge-subtle">
                            <div class="h-2.5 w-16 bg-surface-overlay/20 rounded" />
                            <div class="h-2.5 w-12 bg-surface-overlay/20 rounded" />
                        </div>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}
