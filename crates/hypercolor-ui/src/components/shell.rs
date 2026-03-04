//! App shell — sidebar + header + content area + command palette.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use wasm_bindgen::JsCast;

use crate::api;
use crate::components::sidebar::Sidebar;
use crate::components::status_bar::StatusBar;

/// Top-level layout shell. Sidebar left, header + content right.
#[component]
pub fn Shell(children: Children) -> impl IntoView {
    let (palette_open, set_palette_open) = signal(false);

    // Global keyboard shortcuts
    let navigate = use_navigate();
    let keydown_handler = move |ev: leptos::ev::KeyboardEvent| {
        let key = ev.key();
        let ctrl_or_meta = ev.ctrl_key() || ev.meta_key();

        // Ctrl+K — command palette
        if ctrl_or_meta && key == "k" {
            ev.prevent_default();
            set_palette_open.update(|v| *v = !*v);
            return;
        }

        // Escape — close palette
        if key == "Escape" && palette_open.get() {
            set_palette_open.set(false);
            return;
        }

        // Ctrl+1 — Dashboard
        if ctrl_or_meta && key == "1" {
            ev.prevent_default();
            navigate("/", Default::default());
            return;
        }

        // Ctrl+2 — Effects
        if ctrl_or_meta && key == "2" {
            ev.prevent_default();
            navigate("/effects", Default::default());
        }
    };

    view! {
        <div
            class="flex h-screen bg-layer-0 text-zinc-100 overflow-hidden"
            on:keydown=keydown_handler
            tabindex="-1"
        >
            <Sidebar />
            <div class="flex flex-col flex-1 min-w-0">
                <header class="h-12 flex items-center justify-between px-4 border-b border-white/5 bg-layer-1">
                    <div class="flex items-center gap-3">
                        <span class="text-sm text-zinc-400 font-mono tracking-wide">"HYPERCOLOR"</span>
                        // Quick command palette hint
                        <button
                            class="hidden md:flex items-center gap-2 px-3 py-1 rounded-md bg-white/[0.03] border border-white/5
                                   text-xs text-zinc-600 hover:text-zinc-400 hover:border-white/10 transition-colors cursor-pointer"
                            on:click=move |_| set_palette_open.set(true)
                        >
                            <span>"Search effects..."</span>
                            <kbd class="text-[10px] font-mono bg-white/[0.05] px-1 rounded">"⌘K"</kbd>
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

    // Auto-focus the input
    Effect::new(move |_| {
        if let Some(input) = input_ref.get() {
            let _ = input.focus();
        }
    });

    // Fetch effects for search
    let effects_resource = LocalResource::new(|| api::fetch_effects());

    // Filter effects by query
    let filtered = Memo::new(move |_| {
        let Some(Ok(effects)) = effects_resource.get() else {
            return Vec::new();
        };

        let q = query.get().to_lowercase();
        if q.is_empty() {
            return effects.into_iter().filter(|e| e.runnable).take(10).collect();
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

    let on_close_bg = on_close.clone();
    let on_close_apply = on_close.clone();

    view! {
        <div class="fixed inset-0 z-50 flex items-start justify-center pt-[15vh]">
            // Backdrop
            <div
                class="absolute inset-0 bg-black/60 backdrop-blur-sm"
                on:click=move |_| on_close_bg.run(())
            />

            // Palette
            <div class="relative w-full max-w-lg mx-4 rounded-xl bg-layer-2/95 backdrop-blur-xl border border-white/10
                        shadow-[0_25px_60px_rgba(0,0,0,0.5),0_0_40px_rgba(225,53,255,0.05)]
                        overflow-hidden animate-in">
                // Search input
                <div class="flex items-center gap-3 px-4 py-3 border-b border-white/5">
                    <svg class="w-4 h-4 text-zinc-500 shrink-0" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <circle cx="11" cy="11" r="8"/>
                        <path d="m21 21-4.3-4.3"/>
                    </svg>
                    <input
                        node_ref=input_ref
                        type="text"
                        placeholder="Search effects..."
                        class="flex-1 bg-transparent text-sm text-zinc-100 placeholder-zinc-600 outline-none"
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
                    <kbd class="text-[10px] font-mono text-zinc-600 bg-white/[0.05] px-1.5 py-0.5 rounded">"ESC"</kbd>
                </div>

                // Results
                <div class="max-h-[300px] overflow-y-auto py-1">
                    {move || {
                        let items = filtered.get();
                        if items.is_empty() {
                            view! {
                                <div class="px-4 py-8 text-center text-xs text-zinc-600">
                                    "No matching effects"
                                </div>
                            }.into_any()
                        } else {
                            let on_close = on_close_apply.clone();
                            view! {
                                <div>
                                    {items.into_iter().map(|effect| {
                                        let id = effect.id.clone();
                                        let name = effect.name.clone();
                                        let desc = effect.description.clone();
                                        let category = effect.category.clone();
                                        let on_close = on_close.clone();

                                        view! {
                                            <button
                                                class="w-full flex items-center gap-3 px-4 py-2.5 text-left
                                                       hover:bg-white/[0.05] transition-colors group"
                                                on:click=move |_| {
                                                    let id = id.clone();
                                                    leptos::task::spawn_local(async move {
                                                        let _ = api::apply_effect(&id).await;
                                                    });
                                                    on_close.run(());
                                                }
                                            >
                                                <div class="flex-1 min-w-0">
                                                    <div class="text-sm text-zinc-200 group-hover:text-white truncate">
                                                        {name}
                                                    </div>
                                                    <div class="text-[10px] text-zinc-600 truncate">{desc}</div>
                                                </div>
                                                <span class="text-[10px] text-zinc-600 capitalize shrink-0">{category}</span>
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
