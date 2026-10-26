//! Zone tabs: how every zone picker draws a zone and moves between zones.
//!
//! One anatomy and one keyboard model serve the compact chips on the
//! preview cabinet and Studio's zone strip alike. [`ZoneLabel`] draws a
//! zone as its [`ZoneDot`], its name, and one status glyph with words for
//! screen readers; [`tab_status`] decides that glyph; [`next_tab`] maps
//! the navigation keys; and selection follows focus with one tab in the
//! page's tab order. [`CompactZoneTabs`] is the cabinet's density. The
//! Studio strip wraps the same parts in its editing affordances: rename,
//! the zone menu, drag reorder, and "+".
//!
//! A tab writes [`ZonesContext::focused_zone`], the one record of the
//! target zone, so the preview, the controls, the presets, and effect
//! applies move together. The compact chips hide while the scene has a
//! single LED zone, where there is nothing to choose.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use hypercolor_leptos_ext::events::document as browser_document;
use hypercolor_types::event::LayerHealth;
use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;
use web_sys::KeyboardEvent;

use crate::api;
use crate::app::WsContext;
use crate::icons::*;
use crate::ws::messages::zone_has_degraded_layer;
use crate::zones::ZonesContext;
use crate::zones::surface::Surface;

fn next_zone_tabs_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// The one optional status glyph a zone tab carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabStatus {
    /// Nothing to report.
    Clear,
    /// The zone is disabled.
    Paused,
    /// A layer on the zone failed to render.
    LayerFailed,
    /// The zone holds no devices.
    NoDevices,
}

impl TabStatus {
    /// The words a tab gives its glyph: the hover title, and what screen
    /// readers hear after the zone's name.
    #[must_use]
    pub const fn note(self) -> Option<&'static str> {
        match self {
            Self::Clear => None,
            Self::Paused => Some("Paused"),
            Self::LayerFailed => Some("A layer on this zone failed to render"),
            Self::NoDevices => Some("No devices in this zone yet"),
        }
    }

    /// Whether the glyph is the warning triangle.
    #[must_use]
    pub const fn is_warning(self) -> bool {
        matches!(self, Self::LayerFailed | Self::NoDevices)
    }
}

/// Pick a tab's status glyph. A paused zone says so before anything else,
/// because a disabled zone renders nothing whatever its layers report.
#[must_use]
pub const fn tab_status(enabled: bool, degraded: bool, member_count: usize) -> TabStatus {
    if !enabled {
        TabStatus::Paused
    } else if degraded {
        TabStatus::LayerFailed
    } else if member_count == 0 {
        TabStatus::NoDevices
    } else {
        TabStatus::Clear
    }
}

/// A zone's status from the live scene and the streamed layer health.
#[must_use]
pub fn surface_status(
    surface: &Surface,
    scene: Option<&api::SceneDocument>,
    layer_health: &HashMap<String, LayerHealth>,
) -> TabStatus {
    let Some(scene) = scene else {
        return tab_status(surface.enabled, false, 1);
    };
    let members = scene
        .zones
        .iter()
        .find(|zone| zone.id.to_string() == surface.id)
        .map_or(0, |zone| zone.members.len());
    let degraded = zone_has_degraded_layer(
        layer_health,
        &scene.id.to_string(),
        &surface.id,
        &surface.layer_ids,
    );
    tab_status(surface.enabled, degraded, members)
}

/// The tab a navigation key moves to over `len` tabs in a horizontal
/// strip: the arrows and `[` / `]` step and wrap, Home and End jump, and
/// any other key moves nowhere.
#[must_use]
pub fn next_tab(current: Option<usize>, len: usize, key: &str) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len - 1;
    match key {
        "ArrowRight" | "]" => Some(current.map_or(0, |i| if i >= last { 0 } else { i + 1 })),
        "ArrowLeft" | "[" => Some(current.map_or(last, |i| if i == 0 { last } else { i - 1 })),
        "Home" => Some(0),
        "End" => Some(last),
        _ => None,
    }
}

/// Focus the element with id `dom_id` once it has rendered: a tab the
/// keys just selected, or where a menu hands focus back. Looks the element
/// up by id, so a frame that lands after its view is gone does nothing.
pub fn focus_soon(dom_id: String) {
    request_animation_frame(move || {
        if let Some(tab) = browser_document()
            .and_then(|doc| doc.get_element_by_id(&dom_id))
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = tab.focus();
        }
    });
}

/// A zone's identity dot: its color, or a neutral tone when it has none.
/// Sized to the text around it, so a chip, a tab, a menu row, and a
/// section header each draw the same dot at their own density. The
/// neutral tone reads the semantic variable directly, so it resolves in
/// whichever theme scope the dot renders.
#[component]
pub fn ZoneDot(#[prop(into)] color: Signal<Option<String>>) -> impl IntoView {
    view! {
        <span
            class="h-[0.625em] w-[0.625em] shrink-0 rounded-full"
            class=("bg-[var(--text-tertiary)]/60", move || color.with(Option::is_none))
            style:background-color=move || color.get()
            aria-hidden="true"
        />
    }
}

/// One zone as every zone picker draws it: its dot, its name, and one
/// optional status glyph with words for screen readers. The zone color
/// stays on the dot; it never tints the tab. The glyph tones read the
/// semantic variables directly, like the dot.
///
/// A tab passes `status_id` and points its `aria-describedby` there (see
/// [`status_description`]), so the status describes the tab and its name
/// stays the zone's name as the status comes and goes. Anywhere else the
/// words follow the name.
#[component]
pub fn ZoneLabel(
    #[prop(into)] name: Signal<String>,
    #[prop(into)] color: Signal<Option<String>>,
    #[prop(into)] status: Signal<TabStatus>,
    #[prop(optional, into)] status_id: Option<String>,
) -> impl IntoView {
    view! {
        <span class="flex min-w-0 items-center gap-[0.5em]">
            <ZoneDot color=color />
            <span class="min-w-0 truncate">{move || name.get()}</span>
            {move || {
                let status = status.get();
                let note = status.note()?;
                let (icon, tone) = if status.is_warning() {
                    (LuTriangleAlert, "text-[var(--status-warning)]")
                } else {
                    (LuPause, "text-[var(--text-tertiary)]")
                };
                let words = match status_id.clone() {
                    Some(id) => view! {
                        <span id=id class="sr-only" aria-hidden="true">{note}</span>
                    }
                        .into_any(),
                    None => view! {
                        <span class="sr-only">{format!(", {}", note.to_lowercase())}</span>
                    }
                        .into_any(),
                };
                Some(view! {
                    <span class=format!("flex shrink-0 {tone}")>
                        <Icon icon=icon width="1.1em" height="1.1em" />
                        {words}
                    </span>
                })
            }}
        </span>
    }
}

/// The `aria-describedby` a tab gives its [`ZoneLabel`]'s status words:
/// `status_id` while the zone has a status, nothing while it is clear.
#[must_use]
pub fn status_description(status: TabStatus, status_id: &str) -> Option<String> {
    status.note().map(|_| status_id.to_owned())
}

/// [`ZoneLabel`] for a zone picked outside a tab strip, such as a target
/// hint or a menu row, where only a pause is worth reporting.
#[component]
pub fn ZoneTabLabel(zone: Surface) -> impl IntoView {
    let status = if zone.enabled {
        TabStatus::Clear
    } else {
        TabStatus::Paused
    };
    view! {
        <ZoneLabel
            name=Signal::stored(zone.name)
            color=Signal::stored(zone.color)
            status=Signal::stored(status)
        />
    }
}

/// Compact zone tabs for the preview cabinet overlay.
#[component]
pub fn CompactZoneTabs() -> impl IntoView {
    let zones_ctx = expect_context::<ZonesContext>();
    let ws = use_context::<WsContext>();
    let strip_id = next_zone_tabs_id();
    let tab_id = move |zone_id: &str| format!("zone-tabs-{strip_id}-{zone_id}");
    let selected = Memo::new(move |_| zones_ctx.target_zone().map(|zone| zone.id));

    let select = move |zone_id: String| {
        if zones_ctx.focused_zone.get_untracked().as_deref() != Some(zone_id.as_str()) {
            zones_ctx.focused_zone.set(Some(zone_id));
        }
    };

    let on_keydown = move |ev: KeyboardEvent| {
        if ev.ctrl_key() || ev.meta_key() || ev.alt_key() {
            return;
        }
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
        focus_soon(element_id);
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
                        let status = {
                            let zone_id = zone_id.clone();
                            Memo::new(move |_| {
                                let surface = zones_ctx.zone(&zone_id)?;
                                Some(zones_ctx.active_scene.with(|scene| {
                                    let health = |map: &HashMap<String, LayerHealth>| {
                                        surface_status(&surface, scene.as_ref(), map)
                                    };
                                    ws.map_or_else(
                                        || health(&HashMap::new()),
                                        |ws| ws.layer_health.with(health),
                                    )
                                }))
                            })
                        };
                        let status = Signal::derive(move || status.get().unwrap_or(TabStatus::Clear));
                        let name = zone.name.clone();
                        let title = move || match status.get().note() {
                            Some(note) => format!("Show and edit {name} ({})", note.to_lowercase()),
                            None => format!("Show and edit {name}"),
                        };
                        let enabled = zone.enabled;
                        let status_id = format!("{}-status", tab_id(&zone_id));
                        let described_by = {
                            let status_id = status_id.clone();
                            move || status_description(status.get(), &status_id)
                        };
                        view! {
                            <button
                                type="button"
                                role="tab"
                                id=tab_id(&zone_id)
                                aria-selected=move || is_selected.get().to_string()
                                aria-describedby=described_by
                                tabindex=move || if is_selected.get() { "0" } else { "-1" }
                                title=title
                                class=move || {
                                    let base = "inline-flex min-w-0 max-w-[168px] items-center \
                                                rounded-full border px-2 py-1 text-[10px] font-medium \
                                                backdrop-blur-sm transition-colors duration-150 btn-press \
                                                drop-shadow-[0_1px_3px_rgba(0,0,0,0.85)] \
                                                focus-visible:outline-none focus-visible:ring-1 \
                                                focus-visible:ring-[var(--accent)]";
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
                                <ZoneLabel
                                    name=Signal::stored(zone.name.clone())
                                    color=Signal::stored(zone.color.clone())
                                    status=status
                                    status_id=status_id
                                />
                            </button>
                        }
                    }
                </For>
            </div>
        </Show>
    }
}
