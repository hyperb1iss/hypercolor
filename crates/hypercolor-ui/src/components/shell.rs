//! App shell — sidebar + header + content area + command palette.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::hooks::use_location;
use leptos_router::hooks::use_navigate;
use wasm_bindgen::JsCast;

use crate::app::EffectsContext;
use crate::components::sidebar::Sidebar;
use crate::icons::*;

/// Read the current theme from the DOM.
fn read_theme() -> String {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|el| el.get_attribute("data-theme"))
        .unwrap_or_else(|| "dark".to_string())
}

/// Apply a theme to the DOM and persist to localStorage.
fn apply_theme(theme: &str) {
    if let Some(doc) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = doc.set_attribute("data-theme", theme);
    }
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item("hc-theme", theme);
    }
}

/// Extract dominant hue (0..360) from RGBA pixel data by averaging sampled pixels.
/// Samples every Nth pixel for performance — runs on a throttled timer, not every frame.
fn extract_dominant_hue(frame: &crate::ws::CanvasFrame) -> Option<f64> {
    let pixel_count = frame.pixel_count();
    if pixel_count == 0 {
        return None;
    }

    let step = (pixel_count / 200).max(1); // ~200 samples max
    let mut hue_sin_sum = 0.0_f64;
    let mut hue_cos_sum = 0.0_f64;
    let mut count = 0u32;

    for i in (0..pixel_count).step_by(step) {
        let Some([r, g, b, _]) = frame.rgba_at(i) else {
            continue;
        };
        let r = f64::from(r) / 255.0;
        let g = f64::from(g) / 255.0;
        let b = f64::from(b) / 255.0;

        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let chroma = max - min;

        // Skip near-gray pixels (low chroma = no meaningful hue)
        if chroma < 0.1 {
            continue;
        }

        let hue = if (max - r).abs() < f64::EPSILON {
            60.0 * (((g - b) / chroma) % 6.0)
        } else if (max - g).abs() < f64::EPSILON {
            60.0 * (((b - r) / chroma) + 2.0)
        } else {
            60.0 * (((r - g) / chroma) + 4.0)
        };

        let hue = if hue < 0.0 { hue + 360.0 } else { hue };
        let rad = hue.to_radians();
        hue_sin_sum += rad.sin();
        hue_cos_sum += rad.cos();
        count += 1;
    }

    if count < 5 {
        return None; // Not enough chromatic pixels
    }

    let avg_rad = hue_sin_sum.atan2(hue_cos_sum);
    let avg_hue = avg_rad.to_degrees();
    Some(if avg_hue < 0.0 {
        avg_hue + 360.0
    } else {
        avg_hue
    })
}

/// Shared theme state — provided via context for sidebar and other consumers.
#[derive(Clone, Copy)]
pub struct ThemeContext {
    pub is_dark: Memo<bool>,
    pub toggle: Callback<()>,
}

/// Shared command palette trigger.
#[derive(Clone, Copy)]
pub struct PaletteContext {
    pub open: Callback<()>,
}

/// Top-level layout shell. Sidebar left, header + content right.
#[component]
pub fn Shell(children: Children) -> impl IntoView {
    let (palette_open, set_palette_open) = signal(false);
    let (theme, set_theme) = signal(read_theme());
    let is_dark = Memo::new(move |_| theme.get() == "dark");
    let location = use_location();
    let is_layout_route = Memo::new(move |_| location.pathname.get() == "/layout");

    let toggle_theme = Callback::new(move |()| {
        let next = if is_dark.get() { "light" } else { "dark" };
        apply_theme(next);
        set_theme.set(next.to_string());
    });

    provide_context(ThemeContext {
        is_dark,
        toggle: toggle_theme,
    });

    provide_context(PaletteContext {
        open: Callback::new(move |()| set_palette_open.set(true)),
    });

    // Ambient hue extraction — sample canvas frame ~2x/sec, update --ambient-hue
    let shell_ref = NodeRef::<leptos::html::Div>::new();
    let ws = use_context::<crate::app::WsContext>();
    let (last_hue_update, set_last_hue_update) = signal(0.0_f64);

    if let Some(ws) = ws {
        Effect::new(move |_| {
            let Some(frame) = ws.canvas_frame.get() else {
                return;
            };

            // Throttle to ~2 updates/sec
            let now = js_sys::Date::now();
            if now - last_hue_update.get_untracked() < 500.0 {
                return;
            }
            set_last_hue_update.set(now);

            if let Some(hue) = extract_dominant_hue(&frame)
                && let Some(el) = shell_ref.get() {
                    let html_el: &web_sys::HtmlElement = &el;
                    let style = html_el.style();
                    let _ = style.set_property("--ambient-hue", &format!("{hue:.0}"));
                }
        });
    }

    // Global keyboard shortcuts
    let navigate = use_navigate();
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

        if ctrl_or_meta && key == "1" {
            ev.prevent_default();
            navigate("/", Default::default());
            return;
        }

        if ctrl_or_meta && key == "2" {
            ev.prevent_default();
            navigate("/effects", Default::default());
        }
    };

    view! {
        <div
            node_ref=shell_ref
            class="fixed inset-0 flex min-h-0 bg-surface-base text-fg-primary overflow-hidden noise-overlay"
            on:keydown=keydown_handler
            tabindex="-1"
        >
            <Sidebar />
            <main class=move || {
                if is_layout_route.get() {
                    "flex-1 min-h-0 min-w-0 overflow-hidden"
                } else {
                    "flex-1 min-h-0 min-w-0 overflow-auto p-6"
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

/// Command palette — fuzzy search over effects with one-click apply.
#[component]
fn CommandPalette(#[prop(into)] on_close: Callback<()>) -> impl IntoView {
    let fx = expect_context::<EffectsContext>();
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

    let filtered = Memo::new(move |_| {
        let q = query.get().trim().to_lowercase();
        let effects = fx.effects_index.get();
        if q.is_empty() {
            return effects
                .into_iter()
                .map(|entry| entry.effect)
                .filter(|effect| effect.runnable)
                .take(10)
                .collect();
        }

        effects
            .into_iter()
            .filter(|entry| {
                entry.effect.runnable
                    && (entry.matches_search(&q)
                        || entry.effect.category.to_lowercase().contains(&q))
            })
            .map(|entry| entry.effect)
            .take(10)
            .collect::<Vec<_>>()
    });

    let on_close_bg = on_close;
    let on_close_apply = on_close;

    view! {
        <div class="fixed inset-0 z-50 flex items-start justify-center pt-[12vh] animate-fade-in">
            // Backdrop
            <div
                class="absolute inset-0 bg-black/70 backdrop-blur-md"
                on:click=move |_| on_close_bg.run(())
            />

            // Palette panel
            <div
                class="relative w-full max-w-lg mx-4 rounded-xl glass border border-edge-subtle
                        modal-glow overflow-hidden animate-scale-in"
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
                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                            if let Some(el) = target {
                                set_query.set(el.value());
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
                                    if let Some(effect) = items.get(idx) {
                                        fx.apply_effect(effect.id.clone());
                                        on_close.run(());
                                    }
                                }
                                _ => {}
                            }
                        }
                    />
                    <kbd class="text-[9px] font-mono text-fg-tertiary bg-surface-overlay/40 px-1.5 py-0.5 rounded border border-edge-subtle">"ESC"</kbd>
                </div>

                // Results
                <div class="max-h-[320px] overflow-y-auto py-1" role="listbox">
                    {move || {
                        let items = filtered.get();
                        if items.is_empty() {
                            view! {
                                <div class="px-4 py-10 text-center text-xs text-fg-tertiary">
                                    "No matching effects"
                                </div>
                            }.into_any()
                        } else {
                            let close_cb = on_close_apply;
                            view! {
                                <div>
                                    {items.into_iter().enumerate().map(|(i, effect)| {
                                        let id = effect.id.clone();
                                        let name = effect.name.clone();
                                        let desc = effect.description.clone();
                                        let category = effect.category.clone();
                                        let on_close = close_cb;
                                        let is_selected = move || selected_idx.get() == i;

                                        view! {
                                            <button
                                                class="w-full flex items-center gap-3 px-4 py-2.5 text-left
                                                       hover:bg-electric-purple/[0.05] btn-press group animate-fade-in-up"
                                                style=move || if is_selected() {
                                                    format!("animation-delay: {delay_ms}ms; background: rgba(225, 53, 255, 0.10)", delay_ms = i * 30)
                                                } else {
                                                    format!("animation-delay: {}ms", i * 30)
                                                }
                                                role="option"
                                                aria-selected=move || is_selected().to_string()
                                                on:mouseenter=move |_| set_selected_idx.set(i)
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
