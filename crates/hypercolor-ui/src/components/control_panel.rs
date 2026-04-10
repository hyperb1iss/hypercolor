//! Auto-generated control panel — renders widgets from ControlDefinition metadata.
//! Each control resolves its initial value from live `control_values` (if present),
//! falling back to the definition's `default_value`.

use leptos::ev;
use leptos::portal::Portal;
use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::{UseEventListenerOptions, use_event_listener_with_options};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use wasm_bindgen::prelude::*;

use hypercolor_types::canvas::{linear_to_srgb, srgb_to_linear};
use hypercolor_types::effect::{ControlDefinition, ControlKind, ControlType, ControlValue};

use super::canvas_preview::CanvasPreview;
use super::color_wheel::ColorWheel;
use crate::app::WsContext;
use crate::control_geometry::{
    FrameHandle, FrameRect, clamp_frame_rect, drag_frame_rect, resize_frame_rect,
};
use crate::icons::*;

const QUICK_COLOR_SWATCHES: [&str; 10] = [
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

const SCREEN_CAST_FRAME_CONTROL_IDS: [&str; 4] =
    ["frame_x", "frame_y", "frame_width", "frame_height"];

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScreenCastFrameConfig {
    min_width: f32,
    min_height: f32,
    x_step: f32,
    y_step: f32,
    width_step: f32,
    height_step: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScreenCastInteractionState {
    handle: FrameHandle,
    start_rect: FrameRect,
    start_client_x: f64,
    start_client_y: f64,
}

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
                        let screen_cast_frame_config = screen_cast_frame_config(&items);
                        let visible_items = items
                            .into_iter()
                            .filter(|(def, _)| {
                                screen_cast_frame_config.is_none()
                                    || !is_screen_cast_frame_control(def.control_id())
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

#[component]
fn ScreenCastFrameWidget(
    #[prop(into)] control_values: Signal<HashMap<String, ControlValue>>,
    accent_rgb: String,
    on_change: Callback<(String, serde_json::Value)>,
    frame_config: ScreenCastFrameConfig,
) -> impl IntoView {
    let viewport_ref = NodeRef::<leptos::html::Div>::new();
    let (interaction, set_interaction) = signal(None::<ScreenCastInteractionState>);
    let ws = use_context::<WsContext>();

    let frame_rect = Signal::derive(move || {
        control_values.with(|values| screen_cast_frame_rect(values, frame_config))
    });
    let readout_rect = Signal::derive(move || frame_rect.get());
    let frame_style = Signal::derive({
        let accent_rgb = accent_rgb.clone();
        move || {
            let frame = frame_rect.get();
            format!(
                "left: {:.2}%; top: {:.2}%; width: {:.2}%; height: {:.2}%; \
                 border-color: rgba({}, 0.86); \
                 box-shadow: 0 0 0 999px rgba(5, 6, 12, 0.52), inset 0 0 0 1px rgba(255,255,255,0.06), 0 0 24px rgba({}, 0.18);",
                frame.x * 100.0,
                frame.y * 100.0,
                frame.width * 100.0,
                frame.height * 100.0,
                accent_rgb,
                accent_rgb,
            )
        }
    });
    let preview_style = format!(
        "background:
            radial-gradient(circle at 18% 20%, rgba({0}, 0.22), transparent 30%),
            radial-gradient(circle at 76% 72%, rgba(255, 106, 193, 0.18), transparent 34%),
            linear-gradient(145deg, rgba({0}, 0.12), rgba(10, 9, 16, 0.94) 60%),
            linear-gradient(rgba(255,255,255,0.035) 1px, transparent 1px),
            linear-gradient(90deg, rgba(255,255,255,0.035) 1px, transparent 1px);
         background-size: auto, auto, auto, 12% 12%, 12% 12%;",
        accent_rgb
    );
    let card_style = format!(
        "border-color: rgba({}, 0.12); background: linear-gradient(180deg, rgba({}, 0.08), rgba(10, 9, 16, 0.7));",
        accent_rgb, accent_rgb
    );
    let reset_button_style = format!(
        "border-color: rgba({0}, 0.16); color: rgba({0}, 0.88); background: rgba({0}, 0.08);",
        accent_rgb
    );
    let preview_dot_rgb = accent_rgb.clone();
    let label_style = format!("color: rgba({}, 0.56); letter-spacing: 0.14em;", accent_rgb);
    let pill_style = format!(
        "border-color: rgba({0}, 0.12); background: rgba({0}, 0.08); box-shadow: inset 0 1px 0 rgba(255,255,255,0.03);",
        accent_rgb
    );

    let _drag_move = window_event_listener(ev::mousemove, {
        let on_change = on_change.clone();
        move |ev| {
            let Some(state) = interaction.get_untracked() else {
                return;
            };
            let Some(viewport) = viewport_ref.get_untracked() else {
                return;
            };
            let rect = viewport.get_bounding_client_rect();
            if rect.width() <= 0.0 || rect.height() <= 0.0 {
                return;
            }

            ev.prevent_default();
            let delta_x = (f64::from(ev.client_x()) - state.start_client_x) / rect.width();
            let delta_y = (f64::from(ev.client_y()) - state.start_client_y) / rect.height();
            let next_rect = match state.handle {
                FrameHandle::Move => drag_frame_rect(
                    state.start_rect,
                    delta_x as f32,
                    delta_y as f32,
                    frame_config.min_width,
                    frame_config.min_height,
                ),
                handle => resize_frame_rect(
                    state.start_rect,
                    handle,
                    delta_x as f32,
                    delta_y as f32,
                    frame_config.min_width,
                    frame_config.min_height,
                ),
            };

            emit_screen_cast_frame_update(
                &on_change,
                snap_screen_cast_frame_rect(next_rect, frame_config),
            );
        }
    });

    let _drag_end = window_event_listener(ev::mouseup, move |_| {
        if interaction.get_untracked().is_some() {
            set_interaction.set(None);
        }
    });

    let start_interaction =
        Callback::new(move |(handle, ev): (FrameHandle, web_sys::MouseEvent)| {
            let start_rect = frame_rect.get_untracked();
            ev.prevent_default();
            ev.stop_propagation();
            set_interaction.set(Some(ScreenCastInteractionState {
                handle,
                start_rect,
                start_client_x: f64::from(ev.client_x()),
                start_client_y: f64::from(ev.client_y()),
            }));
        });

    let reset_frame = {
        let on_change = on_change.clone();
        move |_| {
            emit_screen_cast_frame_update(&on_change, FrameRect::new(0.0, 0.0, 1.0, 1.0));
        }
    };

    view! {
        <div class="mb-2 rounded-2xl border p-3 space-y-3" style=card_style>
            <div class="flex items-center gap-2">
                <div>
                    <div class="text-[9px] font-mono uppercase" style=label_style>"Screen Frame"</div>
                    <div class="text-[11px] text-fg-tertiary/70 mt-1">
                        "Drag the crop box or pull its corners to aim the cast."
                    </div>
                </div>
                <div class="flex-1" />
                <button
                    type="button"
                    class="rounded-lg border px-2.5 py-1 text-[10px] font-medium uppercase tracking-[0.12em] transition-all duration-150 hover:scale-[1.02]"
                    style=reset_button_style
                    on:click=reset_frame
                >
                    "Reset"
                </button>
            </div>

            <div class="rounded-[1.5rem] border border-white/[0.06] bg-[#09070f]/90 p-3 shadow-[inset_0_1px_0_rgba(255,255,255,0.04)]">
                <div class="flex items-center gap-2 text-[10px] text-fg-tertiary/55 font-mono uppercase tracking-[0.12em] mb-3">
                    <span class="inline-block h-1.5 w-1.5 rounded-full" style=move || format!("background: rgba({}, 0.85)", preview_dot_rgb) />
                    <span>"Live crop preview"</span>
                </div>
                <div class="mx-auto w-full max-w-[280px]">
                    <div
                        node_ref=viewport_ref
                        class="relative overflow-hidden rounded-[1.25rem] border border-white/[0.08] shadow-[inset_0_1px_0_rgba(255,255,255,0.05)] select-none"
                        style="aspect-ratio: 16 / 9;"
                    >
                        <div class="absolute inset-0" style=preview_style />
                        {ws.map(|ws| {
                            view! {
                                <div class="absolute inset-0">
                                    <CanvasPreview
                                        frame=Signal::derive(move || ws.screen_canvas_frame.get())
                                        fps=Signal::derive(|| 0.0_f32)
                                        fps_target=Signal::derive(|| 0_u32)
                                        max_width="100%".to_string()
                                        aspect_ratio="16 / 9".to_string()
                                        consumer_count=ws.set_screen_preview_consumers
                                    />
                                </div>
                            }
                        })}
                        <div class="absolute inset-[7%] rounded-[1rem] border border-white/[0.05]" />
                        <div class="absolute left-1/2 top-0 bottom-0 w-px bg-white/[0.05] -translate-x-1/2" />
                        <div class="absolute top-1/2 left-0 right-0 h-px bg-white/[0.05] -translate-y-1/2" />

                        <div
                            class="absolute rounded-[1rem] border cursor-grab"
                            class:cursor-grabbing=move || interaction.get().is_some()
                            style=move || frame_style.get()
                            on:mousedown=move |ev| start_interaction.run((FrameHandle::Move, ev))
                        >
                            <div class="absolute inset-0 rounded-[0.95rem] bg-white/[0.035] backdrop-blur-[1px]" />
                            <div class="absolute left-2 top-2 rounded-full bg-black/45 px-2 py-1 text-[9px] font-semibold uppercase tracking-[0.14em] text-white/80">
                                "Screen Cast"
                            </div>

                            <FrameHandleGrip
                                accent_rgb=accent_rgb.clone()
                                class="left-0 top-0 -translate-x-1/2 -translate-y-1/2 cursor-nwse-resize"
                                on_mousedown=Callback::new(move |ev| start_interaction.run((FrameHandle::NorthWest, ev)))
                            />
                            <FrameHandleGrip
                                accent_rgb=accent_rgb.clone()
                                class="right-0 top-0 translate-x-1/2 -translate-y-1/2 cursor-nesw-resize"
                                on_mousedown=Callback::new(move |ev| start_interaction.run((FrameHandle::NorthEast, ev)))
                            />
                            <FrameHandleGrip
                                accent_rgb=accent_rgb.clone()
                                class="left-0 bottom-0 -translate-x-1/2 translate-y-1/2 cursor-nesw-resize"
                                on_mousedown=Callback::new(move |ev| start_interaction.run((FrameHandle::SouthWest, ev)))
                            />
                            <FrameHandleGrip
                                accent_rgb=accent_rgb.clone()
                                class="right-0 bottom-0 translate-x-1/2 translate-y-1/2 cursor-nwse-resize"
                                on_mousedown=Callback::new(move |ev| start_interaction.run((FrameHandle::SouthEast, ev)))
                            />
                        </div>
                    </div>
                </div>
            </div>

            <div class="grid grid-cols-4 gap-2">
                <FrameReadout label="X" value=Signal::derive(move || readout_rect.get().x) pill_style=pill_style.clone() />
                <FrameReadout label="Y" value=Signal::derive(move || readout_rect.get().y) pill_style=pill_style.clone() />
                <FrameReadout label="W" value=Signal::derive(move || readout_rect.get().width) pill_style=pill_style.clone() />
                <FrameReadout label="H" value=Signal::derive(move || readout_rect.get().height) pill_style=pill_style />
            </div>
        </div>
    }
}

#[component]
fn FrameHandleGrip(
    accent_rgb: String,
    class: &'static str,
    on_mousedown: Callback<web_sys::MouseEvent>,
) -> impl IntoView {
    let grip_style = format!(
        "background: rgba({0}, 0.92); box-shadow: 0 0 0 2px rgba(9, 7, 15, 0.88), 0 0 16px rgba({0}, 0.3);",
        accent_rgb
    );

    view! {
        <button
            type="button"
            class=format!("absolute h-3.5 w-3.5 rounded-full border border-white/20 transition-transform duration-150 hover:scale-110 {class}")
            style=grip_style
            on:mousedown=move |ev| on_mousedown.run(ev)
        />
    }
}

#[component]
fn FrameReadout(
    label: &'static str,
    #[prop(into)] value: Signal<f32>,
    pill_style: String,
) -> impl IntoView {
    view! {
        <div class="rounded-xl border px-2.5 py-2" style=pill_style>
            <div class="text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary/55">{label}</div>
            <div class="mt-1 text-xs font-semibold tabular-nums text-fg-secondary">
                {move || format!("{:.2}", value.get())}
            </div>
        </div>
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
fn install_control_dropdown_outside_handler(class_name: String, set_open: WriteSignal<bool>) {
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
fn install_scroll_close_handler(set_open: WriteSignal<bool>) {
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
fn color_picker_panel_style(trigger: Option<web_sys::HtmlButtonElement>) -> String {
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
fn install_scroll_close_handler_for_picker(set_expanded: WriteSignal<Option<String>>) {
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

fn dropdown_panel_style(trigger: Option<web_sys::HtmlButtonElement>) -> String {
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

fn is_screen_cast_frame_control(control_id: &str) -> bool {
    SCREEN_CAST_FRAME_CONTROL_IDS.contains(&control_id)
}

fn screen_cast_frame_config(
    items: &[(ControlDefinition, String)],
) -> Option<ScreenCastFrameConfig> {
    let find = |control_id: &str| {
        items
            .iter()
            .find_map(|(def, _)| (def.control_id() == control_id).then_some(def))
    };

    let frame_x = find("frame_x")?;
    let frame_y = find("frame_y")?;
    let frame_width = find("frame_width")?;
    let frame_height = find("frame_height")?;

    Some(ScreenCastFrameConfig {
        min_width: frame_width.min.unwrap_or(0.05),
        min_height: frame_height.min.unwrap_or(0.05),
        x_step: frame_x.step.unwrap_or(0.01),
        y_step: frame_y.step.unwrap_or(0.01),
        width_step: frame_width.step.unwrap_or(0.01),
        height_step: frame_height.step.unwrap_or(0.01),
    })
}

fn screen_cast_frame_rect(
    values: &HashMap<String, ControlValue>,
    frame_config: ScreenCastFrameConfig,
) -> FrameRect {
    let x = values
        .get("frame_x")
        .and_then(ControlValue::as_f32)
        .unwrap_or(0.0);
    let y = values
        .get("frame_y")
        .and_then(ControlValue::as_f32)
        .unwrap_or(0.0);
    let width = values
        .get("frame_width")
        .and_then(ControlValue::as_f32)
        .unwrap_or(1.0);
    let height = values
        .get("frame_height")
        .and_then(ControlValue::as_f32)
        .unwrap_or(1.0);

    clamp_frame_rect(
        FrameRect::new(x, y, width, height),
        frame_config.min_width,
        frame_config.min_height,
    )
}

fn emit_screen_cast_frame_update(
    on_change: &Callback<(String, serde_json::Value)>,
    rect: FrameRect,
) {
    on_change.run(("frame_x".to_string(), json!(rect.x)));
    on_change.run(("frame_y".to_string(), json!(rect.y)));
    on_change.run(("frame_width".to_string(), json!(rect.width)));
    on_change.run(("frame_height".to_string(), json!(rect.height)));
}

fn snap_screen_cast_frame_rect(rect: FrameRect, frame_config: ScreenCastFrameConfig) -> FrameRect {
    clamp_frame_rect(
        FrameRect::new(
            snap_to_step(rect.x, frame_config.x_step),
            snap_to_step(rect.y, frame_config.y_step),
            snap_to_step(rect.width, frame_config.width_step),
            snap_to_step(rect.height, frame_config.height_step),
        ),
        frame_config.min_width,
        frame_config.min_height,
    )
}

fn snap_to_step(value: f32, step: f32) -> f32 {
    if step <= f32::EPSILON {
        value
    } else {
        (value / step).round() * step
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
        ControlType::Slider => {
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
                            use wasm_bindgen::JsCast;
                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                            if let Some(el) = target
                                && let Ok(v) = el.value().parse::<f32>() {
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
            }.into_any()
        }
        ControlType::Toggle => {
            let initial = matches!(value.get_untracked(), ControlValue::Boolean(true));
            let (checked, set_checked) = signal(initial);
            let control_name = control_id.clone();
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
            }.into_any()
        }
        ControlType::ColorPicker => {
            let initial = control_value_to_hex(&value.get_untracked());
            let (color, set_color) = signal(initial);
            let (hex_input, set_hex_input) = signal(color.get_untracked());
            let picker_id = control_id.clone();
            let is_open = Memo::new({
                let picker_id = picker_id.clone();
                move |_| expanded_picker_id.get().as_deref() == Some(picker_id.as_str())
            });
            let control_name = StoredValue::new(control_id.clone());
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
            }.into_any()
        }
        ControlType::Dropdown => {
            let labels = def.labels.clone();
            let initial = match value.get_untracked() {
                ControlValue::Enum(s) | ControlValue::Text(s) => s,
                _ => labels.first().cloned().unwrap_or_default(),
            };
            let labels_for_sync = labels.clone();
            let dropdown_labels = StoredValue::new(labels.clone());
            let (selected, set_selected) = signal(initial);
            let (dropdown_open, set_dropdown_open) = signal(false);
            let control_name = control_id.clone();
            let dropdown_control_name = StoredValue::new(control_name.clone());
            let dropdown_class = format!("control-dropdown-{}", control_name);
            let dropdown_class_value = StoredValue::new(dropdown_class.clone());
            let trigger_ref = NodeRef::<leptos::html::Button>::new();

            Effect::new(move |_| {
                let next = match value.get() {
                    ControlValue::Enum(s) | ControlValue::Text(s) => s,
                    _ => labels_for_sync.first().cloned().unwrap_or_default(),
                };
                if selected.get_untracked() != next {
                    set_selected.set(next);
                }
            });

            // Click-outside handler
            {
                let dropdown_class = dropdown_class.clone();
                install_control_dropdown_outside_handler(dropdown_class, set_dropdown_open);
            }
            install_scroll_close_handler(set_dropdown_open);

            view! {
                <div
                    class="flex items-center gap-2.5 rounded-lg px-3 py-2 hover:bg-surface-hover/20 transition-colors duration-200 group"
                    class=("relative", move || dropdown_open.get())
                    class=("z-[100]", move || dropdown_open.get())
                    title=tooltip.unwrap_or_default()
                >
                    <Icon icon=icon width="15px" height="15px" style=icon_style.clone() />
                    <label class="text-xs text-fg-secondary font-medium shrink-0 min-w-[80px] max-w-[120px] truncate">{name.clone()}</label>
                    <div class=format!("relative flex-1 min-w-0 {dropdown_class}")>
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
                            <span class="flex-1 min-w-0 text-left truncate text-fg-primary">
                                {move || selected.get()}
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
                            <Portal>
                                <div class=move || dropdown_class_value.get_value()>
                                    <div
                                        class="fixed z-[9999]
                                               rounded-b-xl overflow-hidden
                                               bg-surface-overlay/98 backdrop-blur-xl
                                               border border-t-0 border-edge-subtle
                                               dropdown-glow animate-slide-down
                                               overflow-y-auto scrollbar-dropdown"
                                        style=move || dropdown_panel_style(trigger_ref.get())
                                        on:mousedown=|ev: leptos::ev::MouseEvent| ev.stop_propagation()
                                    >
                                        {move || dropdown_labels.with_value(|labels| {
                                            labels.iter().map(|label| {
                                                let label_val = label.clone();
                                                let display = label.clone();
                                                let m1 = label.clone();
                                                let m2 = label.clone();
                                                let m3 = label.clone();
                                                let m4 = label.clone();
                                                let control_name = dropdown_control_name.get_value();
                                                view! {
                                                    <button
                                                        type="button"
                                                        class="dropdown-option w-full text-left px-3 py-[7px] text-xs cursor-pointer
                                                               flex items-center gap-2"
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
                                                            class=("bg-accent-muted scale-100 opacity-100", move || selected.get() == m3)
                                                            class=("scale-0 opacity-0", move || selected.get() != m4)
                                                        />
                                                        <span class="truncate">{display}</span>
                                                    </button>
                                                }
                                            }).collect_view()
                                        })}
                                    </div>
                                </div>
                            </Portal>
                        </Show>
                    </div>
                </div>
            }.into_any()
        }
        ControlType::TextInput => {
            let initial = match value.get_untracked() {
                ControlValue::Enum(s) | ControlValue::Text(s) => s.clone(),
                _ => String::new(),
            };
            let (text, set_text) = signal(initial);
            let control_name = control_id.clone();

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
            }.into_any()
        }
        ControlType::GradientEditor => {
            view! {
                <div class="flex items-center gap-2.5 rounded-lg px-3 py-2 opacity-40">
                    <Icon icon=icon width="15px" height="15px" style=icon_style.clone() />
                    <label class="text-xs text-fg-secondary font-medium shrink-0 min-w-[80px] max-w-[120px] truncate">{name.clone()}</label>
                    <div class="flex-1 h-5 rounded-lg bg-gradient-to-r from-electric-purple via-neon-cyan to-coral opacity-30" />
                </div>
            }.into_any()
        }
    }
}

fn control_value_to_hex(value: &ControlValue) -> String {
    match value {
        ControlValue::Color([r, g, b, _]) => {
            format!("#{:02x}{:02x}{:02x}", to_byte(*r), to_byte(*g), to_byte(*b))
        }
        ControlValue::Text(hex) if hex.starts_with('#') && hex.len() >= 7 => hex[..7].to_string(),
        _ => "#ffffff".to_string(),
    }
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
