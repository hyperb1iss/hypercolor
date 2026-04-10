//! Auto-generated control panel — renders widgets from ControlDefinition metadata.
//! Each control resolves its initial value from live `control_values` (if present),
//! falling back to the definition's `default_value`.

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::{UseEventListenerOptions, use_event_listener_with_options};
use std::collections::{BTreeMap, HashMap};
use wasm_bindgen::prelude::*;

use hypercolor_types::canvas::{linear_to_srgb, srgb_to_linear};
use hypercolor_types::effect::{ControlDefinition, ControlKind, ControlType, ControlValue};

use crate::icons::*;

mod boolean;
mod color;
mod enum_select;
mod number;
mod screen_cast;
mod text;

use screen_cast::ScreenCastFrameWidget;

// ── Palette / section colors ─────────────────────────────────────────

/// Quick-pick swatches for the color picker.
///
/// These must remain as literal hex strings because the active-swatch detection
/// compares against normalized hex from the live color signal (see `color.rs`).
/// Using CSS `var(...)` here would break equality and leave swatches un-highlighted.
/// The values intentionally mirror the SilkCircuit token palette:
///   `#6000fc` — deep purple (no token)
///   `#e135ff` — `var(--color-electric-purple)`
///   `#ff6ac1` — `var(--color-coral)`
///   `#80ffea` — `var(--color-neon-cyan)`
///   `#f1fa8c` — `var(--color-electric-yellow)`
///   `#50fa7b` — `var(--color-success-green)`
///   `#82aaff` — `var(--color-info-blue)`
///   `#ffffff` — pure white
///   `#ff8c42` — warm orange (no token)
///   `#0a0910` — near-black surface (no token)
pub(super) const QUICK_COLOR_SWATCHES: [&str; 10] = [
    "#6000fc", "#e135ff", "#ff6ac1", "#80ffea", "#f1fa8c", "#50fa7b", "#82aaff", "#ffffff",
    "#ff8c42", "#0a0910",
];

/// Per-section accent colors from the SilkCircuit palette (RGB triplets for `rgba()`).
const SECTION_COLORS: &[&str] = &[
    "128, 255, 234", // neon cyan
    "255, 106, 193", // coral
    "130, 170, 255", // info blue
    "80, 250, 123",  // success green
    "241, 250, 140", // electric yellow
    "225, 53, 255",  // electric purple
];

pub(super) const SCREEN_CAST_FRAME_CONTROL_IDS: [&str; 4] =
    ["frame_x", "frame_y", "frame_width", "frame_height"];

/// Map a control's semantic kind to a Lucide icon.
fn control_icon(kind: &ControlKind, control_type: &ControlType) -> icondata::Icon {
    match kind {
        ControlKind::Color | ControlKind::Hue => LuPalette,
        ControlKind::Boolean => LuToggleLeft,
        ControlKind::Combobox => LuList,
        ControlKind::Sensor => LuCpu,
        ControlKind::Area | ControlKind::Number => match control_type {
            ControlType::Slider => LuGauge,
            _ => LuSettings2,
        },
        ControlKind::Text => LuType,
        ControlKind::Other(_) => match control_type {
            ControlType::Slider => LuGauge,
            ControlType::Toggle => LuToggleLeft,
            ControlType::ColorPicker => LuPalette,
            ControlType::Dropdown => LuList,
            ControlType::TextInput => LuType,
            ControlType::GradientEditor => LuPalette,
        },
    }
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
        let fallback = accent_rgb.get();
        let mut groups: BTreeMap<String, Vec<ControlDefinition>> = BTreeMap::new();
        for def in defs {
            let group = def.group.clone().unwrap_or_else(|| "General".to_string());
            groups.entry(group).or_default().push(def);
        }
        let count = groups.len();
        groups
            .into_iter()
            .enumerate()
            .map(|(i, (group, defs))| {
                let rgb = if count <= 1 {
                    fallback.clone()
                } else {
                    SECTION_COLORS[i % SECTION_COLORS.len()].to_string()
                };
                let items: Vec<(ControlDefinition, String)> =
                    defs.into_iter().map(|d| (d, rgb.clone())).collect();
                (group, rgb, items)
            })
            .collect::<Vec<_>>()
    });

    view! {
        <div class="space-y-1">
            {move || {
                let groups = grouped.get();
                if groups.is_empty() {
                    view! {
                        <div class="text-center py-6">
                            <div class="text-fg-tertiary/40 text-xs">"No controls available"</div>
                        </div>
                    }.into_any()
                } else {
                    let total_groups = groups.len();
                    groups.into_iter().map(|(group, section_rgb, items)| {
                        let screen_cast_frame_config = screen_cast::screen_cast_frame_config(&items);
                        let visible_items = items
                            .into_iter()
                            .filter(|(def, _)| {
                                screen_cast_frame_config.is_none()
                                    || !screen_cast::is_screen_cast_frame_control(def.control_id())
                            })
                            .collect::<Vec<_>>();
                        let show_header = total_groups > 1 && group != "General";
                        view! {
                            <div class="animate-fade-in-up">
                                {show_header.then({
                                    let line_style = format!(
                                        "background: linear-gradient(to right, transparent, rgba({}, 0.25), transparent)",
                                        section_rgb
                                    );
                                    let label_style = format!("color: rgba({}, 0.5)", section_rgb);
                                    move || view! {
                                        <div class="flex items-center gap-2.5 mt-3 mb-1.5 px-1">
                                            <div class="h-px flex-1" style=line_style.clone() />
                                            <h4 class="text-[9px] font-mono uppercase tracking-[0.15em] shrink-0"
                                                style=label_style>
                                                {group.clone()}
                                            </h4>
                                            <div class="h-px flex-1" style=line_style />
                                        </div>
                                    }
                                })}
                                {screen_cast_frame_config.map(|frame_config| {
                                    view! {
                                        <ScreenCastFrameWidget
                                            control_values=control_values
                                            accent_rgb=section_rgb.clone()
                                            on_change=on_change
                                            frame_config
                                        />
                                    }
                                })}
                                {visible_items.into_iter().enumerate().map(|(i, (def, rgb))| {
                                    let control_id = def.control_id().to_owned();
                                    let default_value = def.default_value.clone();
                                    let value = Signal::derive({
                                        let control_id = control_id.clone();
                                        move || {
                                            control_values
                                                .with(|values| values.get(&control_id).cloned())
                                                .unwrap_or_else(|| default_value.clone())
                                        }
                                    });
                                    let delay = format!("animation-delay: {}ms", i * 30);
                                    view! {
                                        <div class="animate-fade-in-up" style=delay>
                                            <ControlWidget def=def value=value accent_rgb=rgb on_change=on_change expanded_picker_id=expanded_picker_id set_expanded_picker_id=set_expanded_picker_id />
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

/// A single control widget, dispatched by ControlType.
#[component]
fn ControlWidget(
    def: ControlDefinition,
    #[prop(into)] value: Signal<ControlValue>,
    accent_rgb: String,
    on_change: Callback<(String, serde_json::Value)>,
    expanded_picker_id: ReadSignal<Option<String>>,
    set_expanded_picker_id: WriteSignal<Option<String>>,
) -> impl IntoView {
    let name = def.name.clone();
    let control_id = def.control_id().to_owned();
    let tooltip = def.tooltip.clone();
    let icon = control_icon(&def.kind, &def.control_type);
    let icon_style = format!(
        "color: rgba({}, 0.6); overflow: visible; flex-shrink: 0",
        accent_rgb
    );

    match def.control_type {
        ControlType::Slider => number::render_slider(
            &def,
            name,
            control_id,
            tooltip,
            icon,
            icon_style,
            accent_rgb,
            value,
            on_change,
        )
        .into_any(),
        ControlType::Toggle => boolean::render_toggle(
            name,
            control_id,
            tooltip,
            icon,
            icon_style,
            accent_rgb,
            value,
            on_change,
        )
        .into_any(),
        ControlType::ColorPicker => color::render_color_picker(
            name,
            control_id,
            tooltip,
            value,
            on_change,
            expanded_picker_id,
            set_expanded_picker_id,
        )
        .into_any(),
        ControlType::Dropdown => enum_select::render_dropdown(
            &def,
            name,
            control_id,
            tooltip,
            icon,
            icon_style,
            value,
            on_change,
        )
        .into_any(),
        ControlType::TextInput => text::render_text_input(
            name,
            control_id,
            tooltip,
            icon,
            icon_style,
            value,
            on_change,
        )
        .into_any(),
        ControlType::GradientEditor => view! {
            <div class="flex items-center gap-2.5 rounded-lg px-3 py-2 opacity-40">
                <Icon icon=icon width="15px" height="15px" style=icon_style.clone() />
                <label class="text-xs text-fg-secondary font-medium shrink-0 min-w-[80px] max-w-[120px] truncate">{name.clone()}</label>
                <div class="flex-1 h-5 rounded-lg bg-gradient-to-r from-electric-purple via-neon-cyan to-coral opacity-30" />
            </div>
        }
        .into_any(),
    }
}

/// Install a window-level mousedown listener that closes the color picker when
/// clicking outside `.color-picker-popover` or `.swatch-glow`.
fn install_click_outside_handler(set_expanded: WriteSignal<Option<String>>) {
    let Some(win) = web_sys::window() else {
        return;
    };

    let _ = use_event_listener_with_options(
        win,
        ev::mousedown,
        move |ev: leptos::ev::MouseEvent| {
            let inside = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .map(|el| {
                    el.closest(".color-picker-popover").ok().flatten().is_some()
                        || el.closest(".swatch-glow").ok().flatten().is_some()
                })
                .unwrap_or(false);

            if !inside {
                set_expanded.set(None);
            }
        },
        UseEventListenerOptions::default().capture(true),
    );
}

/// Install a one-time document-level mousedown listener that closes a specific
/// control dropdown when clicking outside its container.
pub(super) fn install_control_dropdown_outside_handler(
    class_name: String,
    set_open: WriteSignal<bool>,
) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let selector = format!(".{class_name}");
    let _ = use_event_listener_with_options(
        doc,
        ev::mousedown,
        move |ev: leptos::ev::MouseEvent| {
            let inside = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .map(|el| el.closest(&selector).ok().flatten().is_some())
                .unwrap_or(false);

            if !inside {
                set_open.set(false);
            }
        },
        UseEventListenerOptions::default().capture(true),
    );
}

/// Close a dropdown when any ancestor scrolls. The menu is portaled and uses
/// viewport-relative positioning, so scrolling should dismiss it rather than
/// leaving it visually detached from the trigger.
pub(super) fn install_scroll_close_handler(set_open: WriteSignal<bool>) {
    let Some(win) = web_sys::window() else {
        return;
    };

    // Use capture phase to catch scroll events from any descendant.
    let _ = use_event_listener_with_options(
        win,
        ev::scroll,
        move |_: web_sys::Event| {
            set_open.set(false);
        },
        UseEventListenerOptions::default()
            .capture(true)
            .passive(true),
    );
}

/// Compute fixed-position style for the color picker popover, anchored above the
/// swatch trigger button. Falls back to centered if the trigger ref isn't mounted yet.
pub(super) fn color_picker_panel_style(trigger: Option<web_sys::HtmlButtonElement>) -> String {
    let Some(el) = trigger else {
        return String::new();
    };
    let rect = el.get_bounding_client_rect();
    let Some(window) = web_sys::window() else {
        return String::new();
    };

    let viewport_width = window
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(1024.0);
    let viewport_height = window
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(768.0);

    let popover_width = 252.0;
    let margin = 8.0;

    // Center horizontally on the swatch, clamped to viewport
    let ideal_left = rect.left() + rect.width() / 2.0 - popover_width / 2.0;
    let left = ideal_left.clamp(
        margin,
        (viewport_width - popover_width - margin).max(margin),
    );

    // Prefer opening above the trigger; fall back to below if not enough room
    let space_above = rect.top() - margin;
    let space_below = viewport_height - rect.bottom() - margin;
    let open_above = space_above >= 280.0 || space_above > space_below;

    if open_above {
        let bottom = viewport_height - rect.top() + margin;
        format!("left: {left}px; bottom: {bottom}px; width: {popover_width}px")
    } else {
        let top = rect.bottom() + margin;
        format!("left: {left}px; top: {top}px; width: {popover_width}px")
    }
}

/// Close the color picker popover on any ancestor scroll (same rationale as
/// `install_scroll_close_handler` but targets the expanded-picker signal).
pub(super) fn install_scroll_close_handler_for_picker(set_expanded: WriteSignal<Option<String>>) {
    let Some(win) = web_sys::window() else {
        return;
    };

    let _ = use_event_listener_with_options(
        win,
        ev::scroll,
        move |_: web_sys::Event| {
            set_expanded.set(None);
        },
        UseEventListenerOptions::default()
            .capture(true)
            .passive(true),
    );
}

pub(super) fn dropdown_panel_style(trigger: Option<web_sys::HtmlButtonElement>) -> String {
    trigger
        .map(|el| {
            let rect = el.get_bounding_client_rect();
            let Some(window) = web_sys::window() else {
                return String::new();
            };

            let viewport_width = window
                .inner_width()
                .ok()
                .and_then(|value| value.as_f64())
                .unwrap_or(rect.right());
            let viewport_height = window
                .inner_height()
                .ok()
                .and_then(|value| value.as_f64())
                .unwrap_or(rect.bottom());

            let horizontal_margin = 12.0;
            let vertical_margin = 12.0;
            let desired_max_height = 320.0;
            let width = rect.width();
            let max_left = (viewport_width - width - horizontal_margin).max(horizontal_margin);
            let left = rect.left().clamp(horizontal_margin, max_left);
            let available_below = (viewport_height - rect.bottom() - vertical_margin).max(0.0);
            let available_above = (rect.top() - vertical_margin).max(0.0);
            let open_upward = available_below < 160.0 && available_above > available_below;
            let max_height = if open_upward {
                available_above
            } else {
                available_below
            }
            .min(desired_max_height)
            .max(96.0);

            if open_upward {
                let bottom = (viewport_height - rect.top() + 1.0).max(vertical_margin);
                format!(
                    "left: {left}px; bottom: {bottom}px; width: {width}px; max-height: {max_height}px"
                )
            } else {
                let top = (rect.bottom() - 1.0).max(vertical_margin);
                format!("top: {top}px; left: {left}px; width: {width}px; max-height: {max_height}px")
            }
        })
        .unwrap_or_default()
}

// ── Shared hex / color helpers ───────────────────────────────────────

pub(super) fn control_value_to_hex(value: &ControlValue) -> String {
    match value {
        ControlValue::Color([r, g, b, _]) => {
            format!("#{:02x}{:02x}{:02x}", to_byte(*r), to_byte(*g), to_byte(*b))
        }
        ControlValue::Text(hex) if hex.starts_with('#') && hex.len() >= 7 => hex[..7].to_string(),
        _ => "#ffffff".to_string(),
    }
}

pub(super) fn normalize_hex(raw_hex: &str) -> Option<String> {
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

pub(super) fn hex_to_rgba(hex: &str) -> Option<[f32; 4]> {
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
