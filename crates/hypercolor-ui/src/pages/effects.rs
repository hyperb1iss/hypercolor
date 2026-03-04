//! Effects browse page — grid of effect cards with filtering and detail view.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::api;
use crate::app::WsContext;
use crate::components::canvas_preview::CanvasPreview;
use crate::components::control_panel::ControlPanel;
use crate::components::effect_card::EffectCard;

use hypercolor_types::effect::ControlDefinition;

/// Category filter options.
const CATEGORIES: &[&str] = &[
    "all",
    "ambient",
    "audio",
    "gaming",
    "reactive",
    "generative",
    "interactive",
    "productivity",
    "utility",
];

/// Effects browse page with grid, search, category filtering, and live preview.
#[component]
pub fn EffectsPage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let (search, set_search) = signal(String::new());
    let (category_filter, set_category_filter) = signal("all".to_string());
    let (active_effect_id, set_active_effect_id) = signal(None::<String>);
    let (active_controls, set_active_controls) = signal(Vec::<ControlDefinition>::new());

    // Fetch effects list
    let effects_resource = LocalResource::new(api::fetch_effects);

    // Fetch initial active effect
    let active_resource = LocalResource::new(api::fetch_active_effect);

    // Initialize active effect from API
    Effect::new(move |_| {
        if let Some(Ok(Some(active))) = active_resource.get() {
            set_active_effect_id.set(Some(active.id));
            set_active_controls.set(active.controls);
        } else if let Some(Ok(None)) = active_resource.get() {
            set_active_effect_id.set(None);
            set_active_controls.set(Vec::new());
        }
    });

    // Canvas signals from shared WS context
    let canvas_frame = Signal::derive(move || ws.canvas_frame.get());
    let ws_fps = Signal::derive(move || ws.fps.get());

    let controls = Signal::derive(move || active_controls.get());

    // Filter effects
    let filtered_effects = Memo::new(move |_| {
        let Some(Ok(effects)) = effects_resource.get() else {
            return Vec::new();
        };

        let search_term = search.get().to_lowercase();
        let cat = category_filter.get();

        effects
            .into_iter()
            .filter(|e| {
                if cat != "all" && e.category != cat {
                    return false;
                }
                if !search_term.is_empty() {
                    let matches_name = e.name.to_lowercase().contains(&search_term);
                    let matches_desc = e.description.to_lowercase().contains(&search_term);
                    let matches_tags = e
                        .tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&search_term));
                    return matches_name || matches_desc || matches_tags;
                }
                true
            })
            .collect::<Vec<_>>()
    });

    let effect_count = Memo::new(move |_| filtered_effects.get().len());

    // Apply effect handler
    let on_apply = Callback::new(move |id: String| {
        set_active_effect_id.set(Some(id.clone()));
        set_active_controls.set(Vec::new());
        leptos::task::spawn_local(async move {
            let _ = api::apply_effect(&id).await;
            if let Ok(detail) = api::fetch_effect_detail(&id).await {
                set_active_controls.set(detail.controls);
            }
        });
    });

    // Control change handler
    let on_control_change = Callback::new(move |(name, value): (String, serde_json::Value)| {
        if active_effect_id.get().is_some() {
            let controls_json = serde_json::json!({ name: value });
            leptos::task::spawn_local(async move {
                let _ = api::update_controls(&controls_json).await;
            });
        }
    });

    view! {
        <div class="space-y-6">
            // Header with search + filters
            <div class="space-y-4">
                <div class="flex items-center justify-between">
                    <h1 class="text-lg font-medium text-zinc-100">"Effects"</h1>
                    <span class="text-xs font-mono text-zinc-600">
                        {move || effect_count.get()} " effects"
                    </span>
                </div>

                // Search bar
                <div class="relative">
                    <input
                        type="text"
                        placeholder="Search effects..."
                        class="w-full bg-layer-2 border border-white/5 rounded-lg px-4 py-2.5 text-sm text-zinc-200
                               placeholder-zinc-600 focus:outline-none focus:border-electric-purple/30
                               focus:shadow-[0_0_0_1px_rgba(225,53,255,0.15)]"
                        prop:value=move || search.get()
                        on:input=move |ev| {
                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                            if let Some(el) = target {
                                set_search.set(el.value());
                            }
                        }
                    />
                    <div class="absolute right-3 top-1/2 -translate-y-1/2 text-zinc-600 text-xs font-mono">
                        "/"
                    </div>
                </div>

                // Category filter bar
                <div class="flex gap-1.5 flex-wrap">
                    {CATEGORIES.iter().map(|cat| {
                        let cat = cat.to_string();
                        let cat_clone = cat.clone();
                        let is_active = {
                            let cat = cat.clone();
                            Memo::new(move |_| category_filter.get() == cat)
                        };
                        view! {
                            <button
                                class="px-3 py-1 rounded-full text-xs transition-all duration-150 capitalize"
                                class=("bg-electric-purple/20 text-electric-purple border border-electric-purple/30", move || is_active.get())
                                class=("bg-white/[0.03] text-zinc-500 border border-white/5 hover:text-zinc-300 hover:bg-white/[0.06]", move || !is_active.get())
                                on:click=move |_| set_category_filter.set(cat_clone.clone())
                            >
                                {cat.clone()}
                            </button>
                        }
                    }).collect_view()}
                </div>
            </div>

            // Main content area — grid + optional detail panel
            <div class="flex gap-6">
                // Effect grid
                <div class="flex-1 min-w-0">
                    <Suspense fallback=move || view! { <LoadingSkeleton /> }>
                        {move || {
                            let effects = filtered_effects.get();
                            if effects.is_empty() {
                                view! {
                                    <div class="text-center py-16">
                                        <p class="text-zinc-600 text-sm">"No effects found"</p>
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-3">
                                        {effects.into_iter().map(|effect| {
                                            let effect_id = effect.id.clone();
                                            let is_active = Signal::derive(move || {
                                                active_effect_id.get().as_deref() == Some(&effect_id)
                                            });
                                            view! {
                                                <EffectCard
                                                    effect=effect
                                                    is_active=is_active
                                                    on_apply=on_apply
                                                />
                                            }
                                        }).collect_view()}
                                    </div>
                                }.into_any()
                            }
                        }}
                    </Suspense>
                </div>

                // Detail panel — visible when an effect is active
                {move || {
                    active_effect_id.get().map(|_| {
                        view! {
                            <aside class="w-80 shrink-0 space-y-4">
                                // Live preview
                                <div class="rounded-xl bg-layer-2 border border-white/5 overflow-hidden">
                                    <CanvasPreview
                                        frame=canvas_frame
                                        fps=ws_fps
                                        show_fps=true
                                    />
                                </div>

                                // Controls
                                <div class="rounded-xl bg-layer-2 border border-white/5 p-4">
                                    <h3 class="text-xs font-mono uppercase tracking-widest text-zinc-600 mb-3">
                                        "Controls"
                                    </h3>
                                    <ControlPanel
                                        controls=controls
                                        on_change=on_control_change
                                    />
                                </div>

                                // Actions
                                <div class="flex gap-2">
                                    <button
                                        class="flex-1 px-3 py-2 rounded-lg text-xs font-medium bg-error-red/10 text-error-red
                                               border border-error-red/20 hover:bg-error-red/20 transition-colors"
                                        on:click=move |_| {
                                            set_active_effect_id.set(None);
                                            set_active_controls.set(Vec::new());
                                            leptos::task::spawn_local(async move {
                                                let _ = api::stop_effect().await;
                                            });
                                        }
                                    >
                                        "Stop Effect"
                                    </button>
                                </div>
                            </aside>
                        }
                    })
                }}
            </div>
        </div>
    }
}

/// Loading skeleton for the effects grid.
#[component]
fn LoadingSkeleton() -> impl IntoView {
    view! {
        <div class="grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-3">
            {(0..12).map(|_| {
                view! {
                    <div class="rounded-xl border border-white/5 bg-layer-2 p-4 animate-pulse">
                        <div class="flex justify-between mb-2">
                            <div class="h-4 w-32 bg-white/5 rounded" />
                            <div class="h-4 w-16 bg-white/5 rounded-full" />
                        </div>
                        <div class="space-y-1.5 mb-3">
                            <div class="h-3 w-full bg-white/[0.03] rounded" />
                            <div class="h-3 w-2/3 bg-white/[0.03] rounded" />
                        </div>
                        <div class="flex justify-between">
                            <div class="h-3 w-20 bg-white/[0.03] rounded" />
                            <div class="h-3 w-12 bg-white/[0.03] rounded" />
                        </div>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}
