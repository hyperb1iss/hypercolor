//! App shell — sidebar + header + content area + command palette.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::hooks::use_navigate;
use wasm_bindgen::JsCast;

use crate::api;
use crate::components::sidebar::Sidebar;
use crate::components::status_bar::StatusBar;
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
fn extract_dominant_hue(pixels: &[u8]) -> Option<f64> {
    if pixels.len() < 4 {
        return None;
    }

    let step = (pixels.len() / 4 / 200).max(1); // ~200 samples max
    let mut hue_sin_sum = 0.0_f64;
    let mut hue_cos_sum = 0.0_f64;
    let mut count = 0u32;

    for i in (0..pixels.len() / 4).step_by(step) {
        let offset = i * 4;
        let r = f64::from(pixels[offset]) / 255.0;
        let g = f64::from(pixels[offset + 1]) / 255.0;
        let b = f64::from(pixels[offset + 2]) / 255.0;

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
    Some(if avg_hue < 0.0 { avg_hue + 360.0 } else { avg_hue })
}

/// Top-level layout shell. Sidebar left, header + content right.
#[component]
pub fn Shell(children: Children) -> impl IntoView {
    let (palette_open, set_palette_open) = signal(false);
    let (theme, set_theme) = signal(read_theme());
    let is_dark = Memo::new(move |_| theme.get() == "dark");

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

            if let Some(hue) = extract_dominant_hue(&frame.pixels) {
                if let Some(el) = shell_ref.get() {
                    let html_el: &web_sys::HtmlElement = &el;
                    let style = html_el.style();
                    let _ = style.set_property("--ambient-hue", &format!("{hue:.0}"));
                }
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
            class="flex h-screen bg-surface-base text-text-primary overflow-hidden noise-overlay"
            on:keydown=keydown_handler
            tabindex="-1"
        >
            <Sidebar />
            <div class="flex flex-col flex-1 min-w-0">
                // Header
                <header class="h-14 flex items-center justify-between px-6 border-b border-border-subtle bg-surface-raised/80 glass-subtle">
                    <div class="flex items-center gap-4">
                        <span class="text-[11px] text-text-secondary font-mono tracking-[0.2em] uppercase">"Hypercolor"</span>
                        // Command palette trigger
                        <button
                            class="hidden md:flex items-center gap-2.5 px-3 py-1.5 rounded-lg bg-surface-overlay/40 border border-border-subtle
                                   text-xs text-text-tertiary hover:text-text-secondary hover:border-border-default hover:bg-surface-hover/40
                                   btn-press cursor-pointer group"
                            on:click=move |_| set_palette_open.set(true)
                        >
                            <Icon icon=LuSearch width="14px" height="14px" style="color: inherit" />
                            <span class="text-text-tertiary">"Search effects..."</span>
                            <kbd class="text-[9px] font-mono text-text-tertiary bg-surface-overlay/30 px-1.5 py-0.5 rounded border border-border-subtle">"⌘K"</kbd>
                        </button>
                    </div>
                    <div class="flex items-center gap-3">
                        // Theme toggle
                        <button
                            class="p-2 rounded-lg text-text-tertiary hover:text-text-primary hover:bg-surface-hover/40
                                   btn-press transition-colors duration-150"
                            title=move || if is_dark.get() { "Switch to light mode" } else { "Switch to dark mode" }
                            on:click=move |_| {
                                let next = if is_dark.get() { "light" } else { "dark" };
                                apply_theme(next);
                                set_theme.set(next.to_string());
                            }
                        >
                            {move || if is_dark.get() {
                                view! { <Icon icon=LuSun width="16px" height="16px" style="color: inherit" /> }.into_any()
                            } else {
                                view! { <Icon icon=LuMoon width="16px" height="16px" style="color: inherit" /> }.into_any()
                            }}
                        </button>
                        <StatusBar />
                    </div>
                </header>
                <main class="flex-1 overflow-auto p-6">
                    {children()}
                </main>
            </div>

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
    let (query, set_query) = signal(String::new());
    let input_ref = NodeRef::<leptos::html::Input>::new();

    Effect::new(move |_| {
        if let Some(input) = input_ref.get() {
            let _ = input.focus();
        }
    });

    let effects_resource = LocalResource::new(api::fetch_effects);

    let filtered = Memo::new(move |_| {
        let Some(Ok(effects)) = effects_resource.get() else {
            return Vec::new();
        };

        let q = query.get().to_lowercase();
        if q.is_empty() {
            return effects
                .into_iter()
                .filter(|e| e.runnable)
                .take(10)
                .collect();
        }

        effects
            .into_iter()
            .filter(|e| {
                e.runnable
                    && (e.name.to_lowercase().contains(&q)
                        || e.description.to_lowercase().contains(&q)
                        || e.category.contains(&q)
                        || e.tags.iter().any(|t| t.to_lowercase().contains(&q)))
            })
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
            <div class="relative w-full max-w-lg mx-4 rounded-xl glass border border-border-subtle
                        shadow-[0_25px_60px_rgba(0,0,0,0.6),0_0_60px_rgba(225,53,255,0.06)]
                        overflow-hidden animate-scale-in">
                // Search input
                <div class="flex items-center gap-3 px-4 py-3.5 border-b border-border-subtle">
                    <Icon icon=LuSearch width="16px" height="16px" style="color: rgba(225, 53, 255, 0.6); flex-shrink: 0" />
                    <input
                        node_ref=input_ref
                        type="text"
                        placeholder="Search effects..."
                        class="flex-1 bg-transparent text-sm text-text-primary placeholder-text-tertiary outline-none"
                        prop:value=move || query.get()
                        on:input=move |ev| {
                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                            if let Some(el) = target {
                                set_query.set(el.value());
                            }
                        }
                        on:keydown=move |ev| {
                            if ev.key() == "Escape" {
                                on_close.run(());
                            }
                        }
                    />
                    <kbd class="text-[9px] font-mono text-text-tertiary bg-surface-overlay/40 px-1.5 py-0.5 rounded border border-border-subtle">"ESC"</kbd>
                </div>

                // Results
                <div class="max-h-[320px] overflow-y-auto py-1">
                    {move || {
                        let items = filtered.get();
                        if items.is_empty() {
                            view! {
                                <div class="px-4 py-10 text-center text-xs text-text-tertiary">
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
                                        let delay = format!("animation-delay: {}ms", i * 30);

                                        view! {
                                            <button
                                                class="w-full flex items-center gap-3 px-4 py-2.5 text-left
                                                       hover:bg-electric-purple/[0.05] btn-press group animate-fade-in-up"
                                                style=delay
                                                on:click=move |_| {
                                                    let id = id.clone();
                                                    leptos::task::spawn_local(async move {
                                                        let _ = api::apply_effect(&id).await;
                                                    });
                                                    on_close.run(());
                                                }
                                            >
                                                <div class="flex-1 min-w-0">
                                                    <div class="text-sm text-text-secondary group-hover:text-text-primary truncate transition-colors duration-150">{name}</div>
                                                    <div class="text-[10px] text-text-tertiary truncate">{desc}</div>
                                                </div>
                                                <span class="text-[10px] text-text-tertiary capitalize shrink-0 px-2 py-0.5 rounded-full bg-surface-overlay/30">{category}</span>
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
