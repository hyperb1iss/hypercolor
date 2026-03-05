//! App shell — sidebar + header + content area + command palette.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::hooks::use_navigate;
use wasm_bindgen::JsCast;

use crate::api;
use crate::components::sidebar::Sidebar;
use crate::components::status_bar::StatusBar;
use crate::icons::*;

/// Top-level layout shell. Sidebar left, header + content right.
#[component]
pub fn Shell(children: Children) -> impl IntoView {
    let (palette_open, set_palette_open) = signal(false);

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
            class="flex h-screen bg-layer-0 text-fg overflow-hidden noise-overlay"
            on:keydown=keydown_handler
            tabindex="-1"
        >
            <Sidebar />
            <div class="flex flex-col flex-1 min-w-0">
                // Header
                <header class="h-14 flex items-center justify-between px-6 border-b border-white/[0.04] bg-layer-1/80 glass-subtle">
                    <div class="flex items-center gap-4">
                        <span class="text-[11px] text-fg-muted font-mono tracking-[0.2em] uppercase">"Hypercolor"</span>
                        // Command palette trigger
                        <button
                            class="hidden md:flex items-center gap-2.5 px-3 py-1.5 rounded-lg bg-white/[0.02] border border-white/[0.05]
                                   text-xs text-zinc-600 hover:text-zinc-400 hover:border-white/[0.08] hover:bg-white/[0.04]
                                   btn-press cursor-pointer group"
                            on:click=move |_| set_palette_open.set(true)
                        >
                            <Icon icon=LuSearch width="14px" height="14px" style="color: inherit" />
                            <span class="text-zinc-600">"Search effects..."</span>
                            <kbd class="text-[9px] font-mono text-zinc-700 bg-white/[0.04] px-1.5 py-0.5 rounded border border-white/[0.04]">"⌘K"</kbd>
                        </button>
                    </div>
                    <StatusBar />
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
            <div class="relative w-full max-w-lg mx-4 rounded-xl glass border border-white/[0.08]
                        shadow-[0_25px_60px_rgba(0,0,0,0.6),0_0_60px_rgba(225,53,255,0.06)]
                        overflow-hidden animate-scale-in">
                // Search input
                <div class="flex items-center gap-3 px-4 py-3.5 border-b border-white/[0.05]">
                    <Icon icon=LuSearch width="16px" height="16px" style="color: rgba(225, 53, 255, 0.6); flex-shrink: 0" />
                    <input
                        node_ref=input_ref
                        type="text"
                        placeholder="Search effects..."
                        class="flex-1 bg-transparent text-sm text-fg placeholder-fg-dim outline-none"
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
                    <kbd class="text-[9px] font-mono text-zinc-600 bg-white/[0.04] px-1.5 py-0.5 rounded border border-white/[0.04]">"ESC"</kbd>
                </div>

                // Results
                <div class="max-h-[320px] overflow-y-auto py-1">
                    {move || {
                        let items = filtered.get();
                        if items.is_empty() {
                            view! {
                                <div class="px-4 py-10 text-center text-xs text-fg-dim">
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
                                                    <div class="text-sm text-zinc-300 group-hover:text-fg truncate transition-colors duration-150">{name}</div>
                                                    <div class="text-[10px] text-fg-dim truncate">{desc}</div>
                                                </div>
                                                <span class="text-[10px] text-fg-dim capitalize shrink-0 px-2 py-0.5 rounded-full bg-white/[0.03]">{category}</span>
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
