//! App shell — sidebar + header + content area + command palette.

use hypercolor_leptos_ext::events::Input;
use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::hooks::use_location;
use leptos_router::hooks::use_navigate;

use crate::app::{EffectsContext, FrameAnalysisContext};
use crate::components::sidebar::Sidebar;
use crate::icons::*;

/// Top-level layout shell. Sidebar left, header + content right.
#[component]
pub fn Shell(children: Children) -> impl IntoView {
    let (palette_open, set_palette_open) = signal(false);
    let location = use_location();
    let is_layout_route = Memo::new(move |_| location.pathname.get() == "/layout");

    // Ambient hue extraction — driven by the shared frame-analysis pass in app context.
    let shell_ref = NodeRef::<leptos::html::Div>::new();
    let frame_analysis = use_context::<FrameAnalysisContext>();

    if let Some(frame_analysis) = frame_analysis {
        Effect::new(move |_| {
            let Some(analysis) = frame_analysis.live_canvas.get() else {
                return;
            };

            if let Some(el) = shell_ref.get() {
                let html_el: &web_sys::HtmlElement = &el;
                let style = html_el.style();
                let _ =
                    style.set_property("--ambient-hue", &format!("{:.0}", analysis.dominant_hue));
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
