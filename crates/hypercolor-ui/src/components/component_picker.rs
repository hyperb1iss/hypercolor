//! Component picker — searchable dropdown for selecting from the component library.
//! Used only for known hardware definitions (Lian Li fans, Corsair strips, etc.).

use leptos::{ev, portal::Portal, prelude::*};
use leptos_icons::Icon;
use leptos_use::{UseEventListenerOptions, use_event_listener_with_options};
use wasm_bindgen::{JsCast, closure::Closure};

use crate::api;
use crate::icons::*;

// ── Category shape SVGs ─────────────────────────────────────────────────────

fn category_shape_svg(category: &str, size: u32) -> String {
    let s = size;
    let half = s / 2;
    let r = half.saturating_sub(2).max(3);
    let inner_r = r / 3;
    match category {
        "fan" | "aio" | "ring" | "heatsink" => {
            format!(
                r#"<svg viewBox="0 0 {s} {s}" width="{s}" height="{s}"><circle cx="{half}" cy="{half}" r="{r}" fill="none" stroke="currentColor" stroke-width="1.5" opacity="0.6"/><circle cx="{half}" cy="{half}" r="{inner_r}" fill="currentColor" opacity="0.25"/></svg>"#
            )
        }
        "strip" | "radiator" | "case" => {
            let y = half.saturating_sub(2);
            let w = s.saturating_sub(4);
            format!(
                r#"<svg viewBox="0 0 {s} {s}" width="{s}" height="{s}"><rect x="2" y="{y}" width="{w}" height="5" rx="2" fill="none" stroke="currentColor" stroke-width="1.5" opacity="0.6"/></svg>"#
            )
        }
        "strimer" => {
            let y = half.saturating_sub(3);
            let w = s.saturating_sub(4);
            format!(
                r#"<svg viewBox="0 0 {s} {s}" width="{s}" height="{s}"><rect x="2" y="{y}" width="{w}" height="7" rx="1" fill="none" stroke="currentColor" stroke-width="1.5" opacity="0.6" stroke-dasharray="3 1.5"/></svg>"#
            )
        }
        "matrix" => {
            let p = 3_u32;
            let sz = s.saturating_sub(p * 2);
            format!(
                r#"<svg viewBox="0 0 {s} {s}" width="{s}" height="{s}"><rect x="{p}" y="{p}" width="{sz}" height="{sz}" rx="1" fill="none" stroke="currentColor" stroke-width="1.5" opacity="0.6"/></svg>"#
            )
        }
        _ => {
            format!(
                r#"<svg viewBox="0 0 {s} {s}" width="{s}" height="{s}"><circle cx="{half}" cy="{half}" r="{inner_r}" fill="currentColor" opacity="0.35"/></svg>"#
            )
        }
    }
}

// ── Outside click handler ───────────────────────────────────────────────────

fn install_outside_click_handler(set_open: WriteSignal<bool>) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };

    let _ = use_event_listener_with_options(
        doc,
        ev::mousedown,
        move |ev: leptos::ev::MouseEvent| {
            let inside = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .map(|el| {
                    el.closest(".component-picker, .component-picker-panel")
                        .ok()
                        .flatten()
                        .is_some()
                })
                .unwrap_or(false);

            if !inside {
                set_open.set(false);
            }
        },
        UseEventListenerOptions::default(),
    );
}

#[component]
fn ComponentPickerDismissHandler(set_open: WriteSignal<bool>) -> impl IntoView {
    install_outside_click_handler(set_open);
    view! {}
}

fn dropdown_panel_style(trigger: Option<web_sys::HtmlButtonElement>) -> String {
    trigger
        .map(|el| {
            let rect = el.get_bounding_client_rect();
            let Some(window) = web_sys::window() else {
                return String::new();
            };

            let vw = window.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(rect.right());
            let vh = window.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(rect.bottom());

            let margin = 12.0;
            let width = rect.width().max(300.0);
            let max_left = (vw - width - margin).max(margin);
            let left = rect.left().clamp(margin, max_left);
            let below = (vh - rect.bottom() - margin).max(0.0);
            let above = (rect.top() - margin).max(0.0);
            let open_up = below < 180.0 && above > below;
            let max_h = (if open_up { above } else { below }).clamp(120.0, 340.0);

            if open_up {
                let bottom = (vh - rect.top() + 4.0).max(margin);
                format!("left: {left}px; bottom: {bottom}px; width: {width}px; max-height: {max_h}px; z-index: 9999")
            } else {
                let top = (rect.bottom() + 4.0).max(margin);
                format!("top: {top}px; left: {left}px; width: {width}px; max-height: {max_h}px; z-index: 9999")
            }
        })
        .unwrap_or_default()
}

pub(crate) fn filter_components(
    components: &[api::TemplateSummary],
    term: &str,
) -> Vec<api::TemplateSummary> {
    let normalized = term.trim().to_lowercase();
    let mut results: Vec<_> = if normalized.is_empty() {
        components.to_vec()
    } else {
        components
            .iter()
            .filter(|template| {
                template.name.to_lowercase().contains(&normalized)
                    || template.vendor.to_lowercase().contains(&normalized)
                    || template
                        .category
                        .as_str()
                        .to_lowercase()
                        .contains(&normalized)
                    || template.description.to_lowercase().contains(&normalized)
                    || template
                        .tags
                        .iter()
                        .any(|tag| tag.to_lowercase().contains(&normalized))
            })
            .cloned()
            .collect()
    };
    results.sort_by(|left, right| {
        left.vendor
            .cmp(&right.vendor)
            .then_with(|| left.name.cmp(&right.name))
    });
    results
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn selected_result_index(
    results: &[api::TemplateSummary],
    selected_template_id: Option<&str>,
) -> Option<usize> {
    let selected_template_id = selected_template_id?;
    results
        .iter()
        .position(|template| template.id == selected_template_id)
}

fn restore_selected_component_scroll(
    list_ref: NodeRef<leptos::html::Div>,
    selected_template_id: Option<String>,
) {
    let Some(selected_template_id) = selected_template_id else {
        return;
    };
    let Some(list_el) = list_ref.get() else {
        return;
    };

    let items = list_el.get_elements_by_tag_name("button");
    for index in 0..items.length() {
        let Some(item) = items.item(index) else {
            continue;
        };
        if item.get_attribute("data-template-id").as_deref() == Some(selected_template_id.as_str())
        {
            item.scroll_into_view();
            break;
        }
    }
}

// ── Picker component ────────────────────────────────────────────────────────

/// Component library picker — opens a searchable dropdown of known hardware components.
/// When a component is selected, calls `on_select` with the template ID and name.
#[component]
pub fn ComponentPicker(
    components: Vec<api::TemplateSummary>,
    #[prop(into)] on_select: Callback<(String, String)>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let (search, set_search) = signal(String::new());
    let (selected_template_id, set_selected_template_id) = signal(None::<String>);
    let trigger_ref = NodeRef::<leptos::html::Button>::new();
    let search_ref = NodeRef::<leptos::html::Input>::new();
    let list_ref = NodeRef::<leptos::html::Div>::new();
    let components_store = StoredValue::new(components);

    let filtered = Memo::new(move |_| {
        let term = search.get();
        components_store.with_value(|components| filter_components(components, &term))
    });

    Effect::new(move |_| {
        if !open.get() {
            return;
        }

        let search_ref = search_ref.clone();
        let list_ref = list_ref.clone();
        let selected_template_id = selected_template_id.get_untracked();
        if let Some(window) = web_sys::window() {
            let cb = Closure::once_into_js(move || {
                if let Some(input) = search_ref.get() {
                    let _ = input.focus();
                }
                restore_selected_component_scroll(list_ref, selected_template_id);
            });
            let _ = window.set_timeout_with_callback(cb.unchecked_ref());
        }
    });

    view! {
        <div class="component-picker">
            <Show when=move || open.get()>
                <ComponentPickerDismissHandler set_open=set_open />
            </Show>
            <button
                type="button"
                node_ref=trigger_ref
                class="flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-[10px] font-medium transition-all btn-press"
                style="color: rgba(128, 255, 234, 0.6); border: 1px solid rgba(128, 255, 234, 0.1)"
                on:click=move |ev: web_sys::MouseEvent| {
                    ev.stop_propagation();
                    if open.get_untracked() { set_open.set(false); return; }
                    set_open.set(true);
                }
            >
                <Icon icon=LuPlus width="10px" height="10px" />
                "Component"
            </button>

            {move || open.get().then(|| {
                view! {
                    <Portal>
                        <div
                            class="component-picker-panel fixed flex flex-col rounded-xl border border-edge-subtle
                                   bg-surface-overlay shadow-xl dropdown-glow animate-fade-in overflow-hidden"
                            style=move || dropdown_panel_style(trigger_ref.get())
                            on:mousedown=|ev: leptos::ev::MouseEvent| ev.stop_propagation()
                        >
                            <div class="p-1.5 border-b border-edge-subtle">
                                <div class="relative">
                                    <span class="absolute left-2 top-1/2 -translate-y-1/2 pointer-events-none text-fg-tertiary/40">
                                        <Icon icon=LuSearch width="11px" height="11px" />
                                    </span>
                                    <input
                                        type="text"
                                        node_ref=search_ref
                                        placeholder="Search components..."
                                        class="w-full bg-surface-base/60 border border-edge-subtle rounded-lg pl-6 pr-2 py-1
                                               text-[11px] text-fg-primary placeholder-fg-tertiary/40
                                               focus:outline-none focus:border-accent-muted search-glow"
                                        prop:value=move || search.get()
                                        on:input=move |ev| {
                                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                            if let Some(el) = target { set_search.set(el.value()); }
                                        }
                                        on:click=move |ev| ev.stop_propagation()
                                    />
                                </div>
                            </div>

                            <div
                                node_ref=list_ref
                                class="flex-1 overflow-y-auto scrollbar-dropdown"
                            >
                                {move || {
                                    let results = filtered.get();
                                    if results.is_empty() {
                                        return view! {
                                            <div class="px-3 py-4 text-center text-[10px] text-fg-tertiary/40">"No components found"</div>
                                        }.into_any();
                                    }
                                    let selected_index = selected_result_index(
                                        &results,
                                        selected_template_id.get().as_deref(),
                                    );
                                    results.into_iter().enumerate().map(|(index, t)| {
                                        let is_selected = selected_index == Some(index);
                                        let svg = category_shape_svg(t.category.as_str(), 16);
                                        let tid = t.id.clone();
                                        let tname = t.name.clone();
                                        let tname_display = tname.clone();
                                        let vendor = t.vendor.clone();
                                        let led_count = t.led_count;
                                        let category = t.category.as_str().to_string();
                                        view! {
                                            <button
                                                type="button"
                                                data-template-id=tid.clone()
                                                class=if is_selected {
                                                    "w-full flex items-center gap-2 px-2 py-1.5 mx-1 rounded-lg bg-surface-hover/30 ring-1 ring-neon-cyan/20 transition-colors text-left"
                                                } else {
                                                    "w-full flex items-center gap-2 px-2 py-1.5 mx-1 rounded-lg hover:bg-surface-hover/40 transition-colors text-left"
                                                }
                                                style="width: calc(100% - 8px)"
                                                on:click={
                                                    let tid = tid.clone();
                                                    let tname = tname.clone();
                                                    move |ev: web_sys::MouseEvent| {
                                                        ev.stop_propagation();
                                                        set_selected_template_id.set(Some(tid.clone()));
                                                        on_select.run((tid.clone(), tname.clone()));
                                                        set_open.set(false);
                                                    }
                                                }
                                            >
                                                <div class="w-4 h-4 shrink-0 flex items-center justify-center"
                                                     style="color: rgba(128, 255, 234, 0.4)" inner_html=svg />
                                                <div class="flex-1 min-w-0">
                                                    <div class="text-[11px] text-fg-primary leading-tight">{tname_display}</div>
                                                    <div class="text-[9px] text-fg-tertiary/40">
                                                        {vendor} " \u{b7} " <span class="capitalize">{category}</span>
                                                    </div>
                                                </div>
                                                <span class="text-[9px] font-mono tabular-nums text-fg-tertiary/40 shrink-0 px-1 py-0.5 rounded bg-surface-overlay/20">
                                                    {led_count}
                                                </span>
                                            </button>
                                        }
                                    }).collect_view().into_any()
                                }}
                            </div>
                        </div>
                    </Portal>
                }
            })}
        </div>
    }
}
