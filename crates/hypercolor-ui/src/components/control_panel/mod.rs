//! Auto-generated control panel — renders widgets from ControlDefinition metadata.
//! Each control resolves its initial value from live `control_values` (if present),
//! falling back to the definition's `default_value`.

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::{UseEventListenerOptions, use_event_listener_with_options};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use wasm_bindgen::prelude::*;

use hypercolor_types::canvas::{linear_to_srgb, srgb_to_linear};
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, ControlValue, PreviewSource,
};
use hypercolor_types::viewport::ViewportRect;

use crate::app::WsContext;
use crate::icons::*;

mod boolean;
mod color;
mod enum_select;
mod number;
mod sensor;
mod text;
mod viewport_picker;

use viewport_picker::{UrlInputBinding, ViewportPicker};

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

/// Map a control's semantic kind to a Lucide icon.
fn control_icon(kind: &ControlKind, control_type: &ControlType) -> icondata::Icon {
    match kind {
        ControlKind::Color | ControlKind::Hue => LuPalette,
        ControlKind::Boolean => LuToggleLeft,
        ControlKind::Combobox => LuList,
        ControlKind::Sensor => LuCpu,
        ControlKind::Rect => LuSquare,
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
            ControlType::Rect => LuSquare,
        },
    }
}

fn paired_rect_url_controls(
    items: &[(ControlDefinition, String)],
) -> HashMap<String, ControlDefinition> {
    let url_control = items.iter().find_map(|(definition, _)| {
        (matches!(definition.control_type, ControlType::TextInput)
            && definition.control_id().eq_ignore_ascii_case("url"))
        .then(|| definition.clone())
    });
    let Some(url_control) = url_control else {
        return HashMap::new();
    };

    items
        .iter()
        .filter_map(|(definition, _)| {
            (matches!(definition.control_type, ControlType::Rect)
                && definition.preview_source == Some(PreviewSource::WebViewport))
            .then(|| (definition.control_id().to_owned(), url_control.clone()))
        })
        .collect()
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
            <Show when=move || expanded_picker_id.get().is_some()>
                <ColorPickerDismissHandlers
                    expanded_picker_id=expanded_picker_id
                    set_expanded_picker_id=set_expanded_picker_id
                />
            </Show>
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
                        let paired_url_controls = paired_rect_url_controls(&items);
                        let hidden_control_ids: HashSet<String> = paired_url_controls
                            .values()
                            .map(|definition| definition.control_id().to_owned())
                            .collect();
                        let visible_items = items
                            .into_iter()
                            .filter(|(def, _)| {
                                !hidden_control_ids.contains(def.control_id())
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
                                {visible_items.into_iter().enumerate().map(|(i, (def, rgb))| {
                                    let control_id = def.control_id().to_owned();
                                    let default_value = def.default_value.clone();
                                    let paired_url_definition = paired_url_controls
                                        .get(control_id.as_str())
                                        .cloned();
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
                                            <ControlWidget
                                                def=def
                                                value=value
                                                control_values=control_values
                                                paired_url_definition=paired_url_definition
                                                accent_rgb=rgb
                                                on_change=on_change
                                                expanded_picker_id=expanded_picker_id
                                                set_expanded_picker_id=set_expanded_picker_id
                                            />
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

#[component]
fn ColorPickerDismissHandlers(
    expanded_picker_id: ReadSignal<Option<String>>,
    set_expanded_picker_id: WriteSignal<Option<String>>,
) -> impl IntoView {
    install_click_outside_handler(expanded_picker_id, set_expanded_picker_id);
    install_scroll_close_handler_for_picker(expanded_picker_id, set_expanded_picker_id);
    view! {}
}

#[component]
pub fn ControlDropdownDismissHandlers(
    class_name: String,
    is_open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
) -> impl IntoView {
    install_control_dropdown_outside_handler(class_name.clone(), is_open, set_open);
    install_scroll_close_handler(class_name, is_open, set_open);
    view! {}
}

/// A single control widget, dispatched by ControlType.
#[component]
fn ControlWidget(
    def: ControlDefinition,
    #[prop(into)] value: Signal<ControlValue>,
    #[prop(into)] control_values: Signal<HashMap<String, ControlValue>>,
    paired_url_definition: Option<ControlDefinition>,
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
    let ws = use_context::<WsContext>();

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
        ControlType::TextInput => {
            if matches!(def.kind, ControlKind::Sensor) {
                sensor::render_sensor_dropdown(
                    name,
                    control_id,
                    tooltip,
                    icon,
                    icon_style,
                    value,
                    on_change,
                )
                .into_any()
            } else {
                text::render_text_input(
                    name,
                    control_id,
                    tooltip,
                    icon,
                    icon_style,
                    value,
                    on_change,
                )
                .into_any()
            }
        }
        ControlType::GradientEditor => view! {
            <div class="flex items-center gap-2.5 rounded-lg px-3 py-2 opacity-40">
                <Icon icon=icon width="15px" height="15px" style=icon_style.clone() />
                <label class="text-xs text-fg-secondary font-medium shrink-0 min-w-[80px] max-w-[120px] truncate">{name.clone()}</label>
                <div class="flex-1 h-5 rounded-lg bg-gradient-to-r from-electric-purple via-neon-cyan to-coral opacity-30" />
            </div>
        }
        .into_any(),
        ControlType::Rect => {
            let rect_value = Signal::derive(move || match value.get() {
                ControlValue::Rect(rect) => rect,
                _ => ViewportRect::full(),
            });
            let preview_source = def.preview_source;
            let preview_frame = Signal::derive(move || {
                ws.and_then(|ctx| match preview_source {
                    Some(PreviewSource::ScreenCapture) => ctx.screen_canvas_frame.get(),
                    Some(PreviewSource::WebViewport) => ctx.web_viewport_canvas_frame.get(),
                    Some(PreviewSource::EffectCanvas) => ctx.canvas_frame.get(),
                    None => None,
                })
            });
            let preview_consumer_count = ws.and_then(|ctx| match preview_source {
                Some(PreviewSource::ScreenCapture) => Some(ctx.set_screen_preview_consumers),
                Some(PreviewSource::WebViewport) => Some(ctx.set_web_viewport_preview_consumers),
                Some(PreviewSource::EffectCanvas) => Some(ctx.set_preview_consumers),
                None => None,
            });
            let url_input = paired_url_definition.map(|url_definition| {
                let url_control_id = url_definition.control_id().to_owned();
                let placeholder = match &url_definition.default_value {
                    ControlValue::Text(text) | ControlValue::Enum(text) => text.clone(),
                    _ => String::new(),
                };
                let value = Signal::derive({
                    let url_control_id = url_control_id.clone();
                    let placeholder = placeholder.clone();
                    move || {
                        control_values.with(|values| {
                            values
                                .get(&url_control_id)
                                .and_then(|value| match value {
                                    ControlValue::Text(text) | ControlValue::Enum(text) => {
                                        Some(text.clone())
                                    }
                                    _ => None,
                                })
                                .unwrap_or_else(|| placeholder.clone())
                        })
                    }
                });
                UrlInputBinding {
                    label: url_definition.name,
                    value,
                    placeholder,
                    on_commit: Callback::new({
                        let url_control_id = url_control_id.clone();
                        move |next: String| {
                            on_change.run((url_control_id.clone(), json!(next)));
                        }
                    }),
                }
            });

            view! {
                <ViewportPicker
                    control_id=control_id
                    label=name
                    value=rect_value
                    on_change=on_change
                    preview_source=preview_frame
                    preview_consumer_count=preview_consumer_count
                    accent_rgb=accent_rgb
                    aspect_lock=def.aspect_lock
                    url_input=url_input
                />
            }
            .into_any()
        }
    }
}

/// Install a window-level mousedown listener that closes the color picker when
/// clicking outside `.color-picker-popover` or `.swatch-glow`.
fn install_click_outside_handler(
    expanded_picker_id: ReadSignal<Option<String>>,
    set_expanded: WriteSignal<Option<String>>,
) {
    let Some(win) = web_sys::window() else {
        return;
    };

    let _ = use_event_listener_with_options(
        win,
        ev::mousedown,
        move |ev: leptos::ev::MouseEvent| {
            if expanded_picker_id.get_untracked().is_none() {
                return;
            }
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
    is_open: ReadSignal<bool>,
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
            if !is_open.get_untracked() {
                return;
            }
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
/// viewport-relative positioning, so external scrolling should dismiss it
/// rather than leaving it visually detached from the trigger. Scrolls that
/// originate inside the dropdown's own subtree (matched by `class_name`,
/// which is set on both the trigger wrapper and the portaled panel) are
/// ignored — otherwise scrolling the options list would close the menu.
pub(super) fn install_scroll_close_handler(
    class_name: String,
    is_open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
) {
    let Some(win) = web_sys::window() else {
        return;
    };
    let selector = format!(".{class_name}");

    // Use capture phase to catch scroll events from any descendant.
    let _ = use_event_listener_with_options(
        win,
        ev::scroll,
        move |ev: web_sys::Event| {
            if !is_open.get_untracked() {
                return;
            }
            let inside = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .map(|el| el.closest(&selector).ok().flatten().is_some())
                .unwrap_or(false);
            if inside {
                return;
            }
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
pub(super) fn install_scroll_close_handler_for_picker(
    expanded_picker_id: ReadSignal<Option<String>>,
    set_expanded: WriteSignal<Option<String>>,
) {
    let Some(win) = web_sys::window() else {
        return;
    };

    let _ = use_event_listener_with_options(
        win,
        ev::scroll,
        move |_: web_sys::Event| {
            if expanded_picker_id.get_untracked().is_none() {
                return;
            }
            set_expanded.set(None);
        },
        UseEventListenerOptions::default()
            .capture(true)
            .passive(true),
    );
}

pub fn dropdown_panel_style(trigger: Option<web_sys::HtmlButtonElement>) -> String {
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
