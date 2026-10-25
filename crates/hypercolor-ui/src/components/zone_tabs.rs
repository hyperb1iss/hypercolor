//! Zone tabs: the selector for the zone the app is pointed at.
//!
//! A tab writes [`ZonesContext::focused_zone`], the one record of the
//! target zone, so the preview, the controls, the presets, and effect
//! applies move together. The tabs hide while the scene has a single LED
//! zone, where there is nothing to choose.
//!
//! [`ZoneTabLabel`] is one tab's anatomy (identity dot, name, paused
//! glyph), kept apart so every zone selector draws a zone the same way.

use std::sync::atomic::{AtomicU64, Ordering};

use hypercolor_leptos_ext::events::document as browser_document;
use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;
use web_sys::KeyboardEvent;

use crate::components::silk_select::next_highlight;
use crate::icons::*;
use crate::zones::ZonesContext;
use crate::zones::surface::Surface;

fn next_zone_tabs_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// The tab a navigation key moves to over `len` tabs in a horizontal
/// strip: the arrows wrap, Home and End jump, and any other key moves
/// nowhere.
#[must_use]
pub fn next_tab(current: Option<usize>, len: usize, key: &str) -> Option<usize> {
    let list_key = match key {
        "ArrowRight" => "ArrowDown",
        "ArrowLeft" => "ArrowUp",
        "Home" | "End" => key,
        _ => return None,
    };
    next_highlight(current, len, list_key)
}

/// One zone as a tab draws it: the zone's identity dot (neutral when the
/// zone has no color), its name, and a paused glyph while it is disabled.
/// The zone color stays on the dot; it never tints the tab itself. The
/// neutral tones read the semantic variables directly, so they resolve
/// for whichever theme scope the label renders in.
#[component]
pub fn ZoneTabLabel(zone: Surface) -> impl IntoView {
    let dot_style = zone
        .color
        .as_ref()
        .map(|color| format!("background: {color}"))
        .unwrap_or_default();
    let neutral = zone.color.is_none();
    let paused = !zone.enabled;

    view! {
        <span
            class="h-1.5 w-1.5 shrink-0 rounded-full"
            class=("bg-[var(--text-tertiary)]", neutral)
            style=dot_style
            aria-hidden="true"
        />
        <span class="min-w-0 truncate">{zone.name}</span>
        {paused.then(|| view! {
            <span class="flex shrink-0 text-[var(--text-tertiary)]" title="Paused">
                <Icon icon=LuPause width="10px" height="10px" />
                <span class="sr-only">", paused"</span>
            </span>
        })}
    }
}

/// Compact zone tabs for the preview cabinet overlay. Arrow keys move
/// between tabs and select as they go, with one tab in the page's tab
/// order at a time.
#[component]
pub fn CompactZoneTabs() -> impl IntoView {
    let zones_ctx = expect_context::<ZonesContext>();
    let strip_id = next_zone_tabs_id();
    let tab_id = move |zone_id: &str| format!("zone-tabs-{strip_id}-{zone_id}");
    let selected = Memo::new(move |_| zones_ctx.target_zone().map(|zone| zone.id));

    let select = move |zone_id: String| {
        if zones_ctx.focused_zone.get_untracked().as_deref() != Some(zone_id.as_str()) {
            zones_ctx.focused_zone.set(Some(zone_id));
        }
    };

    let on_keydown = move |ev: KeyboardEvent| {
        let zone_ids = zones_ctx
            .led_zones
            .with_untracked(|zones| zones.iter().map(|zone| zone.id.clone()).collect::<Vec<_>>());
        let current = selected
            .get_untracked()
            .and_then(|id| zone_ids.iter().position(|zone_id| *zone_id == id));
        let Some(next) = next_tab(current, zone_ids.len(), &ev.key()) else {
            return;
        };
        ev.prevent_default();
        let Some(zone_id) = zone_ids.get(next).cloned() else {
            return;
        };
        let element_id = tab_id(&zone_id);
        select(zone_id);
        if let Some(tab) = browser_document()
            .and_then(|doc| doc.get_element_by_id(&element_id))
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = tab.focus();
        }
    };

    view! {
        <Show when=move || zones_ctx.multi_zone.get()>
            // The cabinet is a black canvas in both themes, so its tabs
            // take the dark theme's tokens. Tailwind's color aliases are
            // resolved at the document root, so the tab colors read the
            // semantic variables directly to pick up this scope.
            <div
                class="pointer-events-auto flex flex-wrap items-center gap-1.5"
                data-theme="dark"
                role="tablist"
                aria-label="Zone"
                on:keydown=on_keydown
            >
                // Keyed by what a tab draws, so a layer change in a zone
                // never rebuilds the strip under a focused tab.
                <For
                    each=move || zones_ctx.led_zones.get()
                    key=|zone| (zone.id.clone(), zone.name.clone(), zone.color.clone(), zone.enabled)
                    let:zone
                >
                    {
                        let zone_id = zone.id.clone();
                        let is_selected = {
                            let zone_id = zone_id.clone();
                            Signal::derive(move || {
                                selected.with(|selected| selected.as_deref() == Some(zone_id.as_str()))
                            })
                        };
                        let enabled = zone.enabled;
                        let title = if enabled {
                            format!("Show and edit {}", zone.name)
                        } else {
                            format!("Show and edit {} (paused)", zone.name)
                        };
                        view! {
                            <button
                                type="button"
                                role="tab"
                                id=tab_id(&zone_id)
                                aria-selected=move || is_selected.get().to_string()
                                tabindex=move || if is_selected.get() { "0" } else { "-1" }
                                title=title
                                class=move || {
                                    let base = "inline-flex min-w-0 max-w-[168px] items-center gap-1.5 \
                                                rounded-full border px-2 py-1 text-[10px] font-medium \
                                                backdrop-blur-sm transition-colors duration-150 btn-press \
                                                drop-shadow-[0_1px_3px_rgba(0,0,0,0.85)] \
                                                focus-visible:outline-none focus-visible:ring-1 \
                                                focus-visible:ring-accent/60";
                                    let state = if is_selected.get() {
                                        "border-[var(--accent)] bg-[var(--accent-muted)] \
                                         text-[var(--text-primary)] \
                                         shadow-[0_0_10px_var(--glow-accent-50)]"
                                    } else {
                                        "border-[var(--border-subtle)] bg-black/45 \
                                         text-[var(--text-secondary)] hover:border-[var(--border-strong)] \
                                         hover:bg-black/60 hover:text-[var(--text-primary)]"
                                    };
                                    // Dimming rides in the same string: a
                                    // reactive `class` rebuild replaces the
                                    // whole attribute.
                                    let paused = if enabled { "" } else { " opacity-70" };
                                    format!("{base} {state}{paused}")
                                }
                                on:click=move |_| select(zone_id.clone())
                            >
                                <ZoneTabLabel zone=zone />
                            </button>
                        }
                    }
                </For>
            </div>
        </Show>
    }
}
