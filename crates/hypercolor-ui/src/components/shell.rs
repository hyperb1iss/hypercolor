//! App shell — sidebar + header + content area + command palette.

use hypercolor_leptos_ext::events::Input;
use hypercolor_leptos_ext::events::document as browser_document;
use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::hooks::use_location;
use leptos_router::hooks::use_navigate;
use wasm_bindgen::JsCast;

use hypercolor_types::scene::SceneMutationMode;

use crate::app::{EffectsContext, FrameAnalysisContext, StudioFlag};
use crate::components::page_search_bar::PAGE_SEARCH_INPUT_ID;
use crate::components::scene_switcher::active_saved_scene_id;
use crate::components::sidebar::Sidebar;
use crate::icons::*;
use crate::zones::{ScenesContext, ZonesContext};

/// Path for a `Ctrl/Cmd+<digit>` nav shortcut, mirroring the sidebar's nav
/// order for the active nav set (Spec 65 §5.1 swaps Studio/Media in for
/// Assets/Layout/Displays when the beta flag is on).
#[must_use]
pub fn nav_shortcut_path(studio_ui: bool, key: &str) -> Option<&'static str> {
    const BASE: [&str; 8] = [
        "/",
        "/effects",
        "/assets",
        "/layout",
        "/devices",
        "/displays",
        "/capture",
        "/settings",
    ];
    const STUDIO: [&str; 7] = [
        "/",
        "/effects",
        "/studio",
        "/media",
        "/devices",
        "/capture",
        "/settings",
    ];

    let paths: &[&str] = if studio_ui { &STUDIO } else { &BASE };
    let digit = key.parse::<usize>().ok()?;
    (1..=paths.len()).contains(&digit).then(|| paths[digit - 1])
}

/// Whether the event originated from a text-entry surface, where global
/// single-key shortcuts (like `/`) must stay inert.
fn targets_editable(ev: &leptos::ev::KeyboardEvent) -> bool {
    let Some(target) = ev.target() else {
        return false;
    };
    let Some(el) = target.dyn_ref::<web_sys::HtmlElement>() else {
        return false;
    };
    let tag = el.tag_name().to_ascii_lowercase();
    tag == "input" || tag == "textarea" || tag == "select" || el.is_content_editable()
}

/// Focus the current page's search bar (the `/` kbd hint in `PageSearchBar`).
fn focus_page_search() {
    if let Some(doc) = browser_document()
        && let Some(el) = doc.get_element_by_id(PAGE_SEARCH_INPUT_ID)
        && let Some(input) = el.dyn_ref::<web_sys::HtmlElement>()
    {
        let _ = input.focus();
    }
}

/// Top-level layout shell. Sidebar left, header + content right.
#[component]
pub fn Shell(children: Children) -> impl IntoView {
    let (palette_open, set_palette_open) = signal(false);
    let location = use_location();
    let is_layout_route = Memo::new(move |_| location.pathname.get() == "/layout");

    // Ambient hue extraction — driven by the shared frame-analysis pass in
    // app context. The hue is written to `:root` (not the shell div):
    // custom-property `var()` substitution happens where a property is
    // *declared*, and every `--ambient-*` token plus the scrollbar tints are
    // declared in `:root` blocks of tokens/semantic.css, so a hue set any
    // lower in the tree would never reach them.
    let frame_analysis = use_context::<FrameAnalysisContext>();
    let last_ambient_hue = StoredValue::new(None::<i16>);

    if let Some(frame_analysis) = frame_analysis {
        Effect::new(move |_| {
            let Some(analysis) = frame_analysis.live_canvas.get() else {
                return;
            };
            let hue = analysis.dominant_hue.round() as i16;
            if last_ambient_hue.get_value() == Some(hue) {
                return;
            }

            if let Some(doc) = browser_document()
                && let Some(root) = doc.document_element()
                && let Some(html_el) = root.dyn_ref::<web_sys::HtmlElement>()
            {
                let _ = html_el
                    .style()
                    .set_property("--ambient-hue", &hue.to_string());
                last_ambient_hue.set_value(Some(hue));
            }
        });
    }

    // Global keyboard shortcuts
    let navigate = use_navigate();
    let studio_flag = expect_context::<StudioFlag>();
    let keydown_handler = move |ev: leptos::ev::KeyboardEvent| {
        let key = ev.key();
        let ctrl_or_meta = ev.ctrl_key() || ev.meta_key();

        if ctrl_or_meta && key == "k" {
            ev.prevent_default();
            set_palette_open.update(|v| *v = !*v);
            return;
        }

        if key == "Escape" && palette_open.get() {
            set_palette_open.set(false);
            return;
        }

        // Ctrl/Cmd+1..7 jump straight to a nav page in sidebar order.
        if ctrl_or_meta
            && let Some(path) = nav_shortcut_path(studio_flag.enabled.get_untracked(), &key)
        {
            ev.prevent_default();
            navigate(path, Default::default());
            return;
        }

        // `/` focuses the current page's search bar, unless the user is
        // already typing somewhere.
        if key == "/" && !ctrl_or_meta && !ev.alt_key() && !targets_editable(&ev) {
            ev.prevent_default();
            focus_page_search();
        }
    };

    view! {
        <div
            class="fixed inset-0 flex min-h-0 bg-surface-base text-fg-primary overflow-hidden noise-overlay"
            on:keydown=keydown_handler
            tabindex="-1"
        >
            <Sidebar />
            <main class=move || {
                if is_layout_route.get() {
                    "flex-1 min-h-0 min-w-0 overflow-hidden"
                } else {
                    "flex-1 min-h-0 min-w-0 overflow-auto"
                }
            }>
                {children()}
            </main>

            // Command palette overlay
            {move || palette_open.get().then(|| {
                view! { <CommandPalette on_close=move || set_palette_open.set(false) /> }
            })}
        </div>
    }
}

/// One command palette result row: an effect (Enter applies it) or a
/// saved scene (Enter activates it; a no-op when already active).
#[derive(Clone, PartialEq)]
enum PaletteEntry {
    Effect(crate::api::EffectSummary),
    Scene {
        id: String,
        name: String,
        locked: bool,
        active: bool,
    },
}

/// Command palette — fuzzy search over effects and scenes with one-click
/// apply/activate.
#[component]
fn CommandPalette(#[prop(into)] on_close: Callback<()>) -> impl IntoView {
    let fx = expect_context::<EffectsContext>();
    let scenes_ctx = expect_context::<ScenesContext>();
    let zones_ctx = expect_context::<ZonesContext>();
    let (query, set_query) = signal(String::new());
    let (selected_idx, set_selected_idx) = signal(0_usize);
    let input_ref = NodeRef::<leptos::html::Input>::new();

    Effect::new(move |_| {
        if let Some(input) = input_ref.get() {
            let _ = input.focus();
        }
    });

    // Reset selection when query changes
    Effect::new(move |_| {
        query.track();
        set_selected_idx.set(0);
    });

    // Keep the keyboard-selected row visible while arrowing through a list
    // longer than the panel. Nearest-edge alignment, so mouse-driven
    // selection (already on-screen) never causes a scroll jump.
    Effect::new(move |_| {
        let idx = selected_idx.get();
        if let Some(doc) = browser_document()
            && let Some(el) = doc.get_element_by_id(&format!("palette-opt-{idx}"))
        {
            el.scroll_into_view_with_bool(false);
        }
    });

    let filtered = Memo::new(move |_| {
        let q = query.get().trim().to_lowercase();
        let effects = fx.effects_index.get();
        if q.is_empty() {
            return effects
                .into_iter()
                .map(|entry| entry.effect)
                .filter(|effect| effect.runnable)
                .take(10)
                .map(PaletteEntry::Effect)
                .collect();
        }

        let mut entries = effects
            .into_iter()
            .filter(|entry| {
                entry.effect.runnable
                    && (entry.matches_search(&q)
                        || entry.effect.category.to_lowercase().contains(&q))
            })
            .map(|entry| PaletteEntry::Effect(entry.effect))
            .take(10)
            .collect::<Vec<_>>();

        // Scenes match by name and append after the effects; the active
        // scene is marked so Enter on it is an honest no-op.
        let active_id = scenes_ctx
            .active
            .with(|active| active_saved_scene_id(active.as_ref()).map(str::to_owned));
        entries.extend(
            scenes_ctx
                .scenes
                .get()
                .into_iter()
                .filter(|scene| scene.name.to_lowercase().contains(&q))
                .take(4)
                .map(|scene| PaletteEntry::Scene {
                    active: active_id.as_deref() == Some(scene.id.as_str()),
                    locked: scene.mutation_mode == SceneMutationMode::Snapshot,
                    id: scene.id,
                    name: scene.name,
                }),
        );
        entries
    });

    // Run one palette entry: apply the effect or activate the scene.
    let run_entry = move |entry: &PaletteEntry| match entry {
        PaletteEntry::Effect(effect) => {
            fx.apply_effect(effect.id.clone());
            on_close.run(());
        }
        PaletteEntry::Scene { id, active, .. } => {
            if !active {
                scenes_ctx.activate.run(id.clone());
                on_close.run(());
            }
        }
    };

    let on_close_bg = on_close;
    let on_close_apply = on_close;

    view! {
        <div class="fixed inset-0 z-50 flex items-start justify-center pt-[12vh] animate-enter-fade">
            // Backdrop
            <div
                class="absolute inset-0 bg-black/70 backdrop-blur-md"
                on:click=move |_| on_close_bg.run(())
            />

            // Palette panel
            <div
                class="relative w-full max-w-lg mx-4 rounded-xl glass border border-edge-subtle
                        modal-glow overflow-hidden animate-enter-scale"
                role="dialog"
                aria-label="Command palette"
            >
                // Search input
                <div class="flex items-center gap-3 px-4 py-3.5 border-b border-edge-subtle">
                    <Icon icon=LuSearch width="16px" height="16px" style="color: rgba(225, 53, 255, 0.6); flex-shrink: 0" />
                    <input
                        node_ref=input_ref
                        type="text"
                        placeholder="Search effects..."
                        class="flex-1 bg-transparent text-sm text-fg-primary placeholder-fg-tertiary outline-none"
                        role="combobox"
                        aria-expanded="true"
                        aria-autocomplete="list"
                        prop:value=move || query.get()
                        on:input=move |ev| {
                            let event = Input::from_event(ev);
                            if let Some(value) = event.value_string() {
                                set_query.set(value);
                            }
                        }
                        on:keydown=move |ev| {
                            let key = ev.key();
                            match key.as_str() {
                                "Escape" => on_close.run(()),
                                "ArrowDown" => {
                                    ev.prevent_default();
                                    let count = filtered.get().len();
                                    if count > 0 {
                                        set_selected_idx.update(|i| *i = (*i + 1) % count);
                                    }
                                }
                                "ArrowUp" => {
                                    ev.prevent_default();
                                    let count = filtered.get().len();
                                    if count > 0 {
                                        set_selected_idx.update(|i| {
                                            *i = if *i == 0 { count - 1 } else { *i - 1 };
                                        });
                                    }
                                }
                                "Enter" => {
                                    let items = filtered.get();
                                    let idx = selected_idx.get();
                                    if let Some(entry) = items.get(idx) {
                                        run_entry(entry);
                                    }
                                }
                                _ => {}
                            }
                        }
                    />
                    // Target hint — multi-zone scenes name the zone an
                    // apply lands in, so the palette never fires blind.
                    {move || {
                        zones_ctx
                            .multi_zone
                            .get()
                            .then(|| zones_ctx.target_zone())
                            .flatten()
                            .map(|zone| {
                                let dot = zone
                                    .color
                                    .clone()
                                    .unwrap_or_else(|| "var(--color-electric-purple)".to_owned());
                                view! {
                                    <span
                                        class="inline-flex items-center gap-1.5 shrink-0 text-[9px] \
                                               font-mono px-2 py-0.5 rounded-full border \
                                               border-edge-subtle bg-surface-overlay/40 text-fg-tertiary"
                                        title="Effects apply to this zone"
                                    >
                                        <span
                                            class="w-1.5 h-1.5 rounded-full shrink-0"
                                            style:background=dot
                                        />
                                        {format!("applies to: {}", zone.name)}
                                    </span>
                                }
                            })
                    }}
                    <kbd class="text-[9px] font-mono text-fg-tertiary bg-surface-overlay/40 px-1.5 py-0.5 rounded border border-edge-subtle">"ESC"</kbd>
                </div>

                // Results
                <div class="max-h-[320px] overflow-y-auto py-1" role="listbox">
                    {move || {
                        let items = filtered.get();
                        if items.is_empty() {
                            view! {
                                <div class="px-4 py-10 text-center text-xs text-fg-tertiary">
                                    "No matching effects or scenes"
                                </div>
                            }.into_any()
                        } else {
                            let close_cb = on_close_apply;
                            view! {
                                <div>
                                    {items.into_iter().enumerate().map(|(i, entry)| {
                                        let on_close = close_cb;
                                        let is_selected = move || selected_idx.get() == i;
                                        let row_style = move || if is_selected() {
                                            format!("animation-delay: {delay_ms}ms; background: rgba(225, 53, 255, 0.10)", delay_ms = i * 30)
                                        } else {
                                            format!("animation-delay: {}ms", i * 30)
                                        };

                                        match entry {
                                            PaletteEntry::Effect(effect) => {
                                                let id = effect.id.clone();
                                                let name = effect.name.clone();
                                                let desc = effect.description.clone();
                                                let category = effect.category.clone();

                                                view! {
                                                    <button
                                                        class="w-full flex items-center gap-3 px-4 py-2.5 text-left
                                                               hover:bg-electric-purple/[0.05] btn-press group animate-enter-up"
                                                        style=row_style
                                                        id=format!("palette-opt-{i}")
                                                        role="option"
                                                        aria-selected=move || is_selected().to_string()
                                                        // `mousemove`, not `mouseenter`: arrow-key scrolling
                                                        // slides rows under a stationary cursor, and the
                                                        // resulting enter events would fight the keyboard.
                                                        on:mousemove=move |_| {
                                                            if selected_idx.get_untracked() != i {
                                                                set_selected_idx.set(i);
                                                            }
                                                        }
                                                        on:click=move |_| {
                                                            fx.apply_effect(id.clone());
                                                            on_close.run(());
                                                        }
                                                    >
                                                        <div class="flex-1 min-w-0">
                                                            <div class="text-sm text-fg-secondary group-hover:text-fg-primary truncate transition-colors duration-150">{name}</div>
                                                            <div class="text-[10px] text-fg-tertiary truncate">{desc}</div>
                                                        </div>
                                                        <span class="text-[10px] text-fg-tertiary capitalize shrink-0 px-2 py-0.5 rounded-full bg-surface-overlay/30">{category}</span>
                                                    </button>
                                                }.into_any()
                                            }
                                            PaletteEntry::Scene { id, name, locked, active } => {
                                                view! {
                                                    <button
                                                        class="w-full flex items-center gap-3 px-4 py-2.5 text-left
                                                               hover:bg-electric-purple/[0.05] btn-press group animate-enter-up"
                                                        style=row_style
                                                        id=format!("palette-opt-{i}")
                                                        role="option"
                                                        aria-selected=move || is_selected().to_string()
                                                        on:mousemove=move |_| {
                                                            if selected_idx.get_untracked() != i {
                                                                set_selected_idx.set(i);
                                                            }
                                                        }
                                                        on:click=move |_| {
                                                            if active {
                                                                return;
                                                            }
                                                            scenes_ctx.activate.run(id.clone());
                                                            on_close.run(());
                                                        }
                                                    >
                                                        <div class="flex-1 min-w-0 flex items-center gap-2">
                                                            {active.then(|| view! {
                                                                <span class="flex shrink-0 text-accent">
                                                                    <Icon icon=LuCheck width="13px" height="13px" />
                                                                </span>
                                                            })}
                                                            <span class="text-sm text-fg-secondary group-hover:text-fg-primary truncate transition-colors duration-150">
                                                                {name}
                                                            </span>
                                                            {locked.then(|| view! {
                                                                <span
                                                                    class="flex shrink-0 text-electric-yellow/70"
                                                                    title="Snapshot-locked scene"
                                                                >
                                                                    <Icon icon=LuLock width="11px" height="11px" />
                                                                </span>
                                                            })}
                                                        </div>
                                                        <span
                                                            class="text-[10px] shrink-0 px-2 py-0.5 rounded-full"
                                                            style="background: rgba(225, 53, 255, 0.10); color: var(--color-electric-purple)"
                                                        >
                                                            "Scene"
                                                        </span>
                                                    </button>
                                                }.into_any()
                                            }
                                        }
                                    }).collect_view()}
                                </div>
                            }.into_any()
                        }
                    }}
                </div>
            </div>
        </div>
    }
}
