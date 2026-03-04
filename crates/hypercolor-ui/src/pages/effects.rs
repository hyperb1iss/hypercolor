//! Effects browse page — grid of effect cards with filtering and detail panel.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::api;
use crate::app::{EffectsContext, WsContext};
use crate::components::canvas_preview::CanvasPreview;
use crate::components::control_panel::ControlPanel;
use crate::components::effect_card::EffectCard;

/// Category → accent RGB string for inline styles.
fn category_accent_rgb(category: &str) -> &'static str {
    match category {
        "ambient" => "128, 255, 234",
        "audio" => "255, 106, 193",
        "gaming" => "225, 53, 255",
        "reactive" => "241, 250, 140",
        "generative" => "80, 250, 123",
        "interactive" => "130, 170, 255",
        "productivity" => "255, 153, 255",
        "utility" => "139, 133, 160",
        _ => "225, 53, 255",
    }
}

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

/// Effects browse page with compact grid, search, category filtering, and live preview.
#[component]
pub fn EffectsPage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let fx = expect_context::<EffectsContext>();

    let (search, set_search) = signal(String::new());
    let (category_filter, set_category_filter) = signal("all".to_string());

    let canvas_frame = Signal::derive(move || ws.canvas_frame.get());
    let ws_fps = Signal::derive(move || ws.fps.get());
    let controls = Signal::derive(move || fx.active_controls.get());
    let control_values = Signal::derive(move || fx.active_control_values.get());
    let accent_rgb = Signal::derive(move || {
        category_accent_rgb(&fx.active_effect_category.get()).to_string()
    });

    let has_active = Memo::new(move |_| fx.active_effect_id.get().is_some());

    // Filter effects
    let filtered_effects = Memo::new(move |_| {
        let Some(Ok(effects)) = fx.effects_resource.get() else {
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

    // Apply effect handler — delegates to shared context
    let on_apply = Callback::new(move |id: String| {
        fx.apply_effect(id);
    });

    // Control change handler
    let on_control_change = Callback::new(move |(name, value): (String, serde_json::Value)| {
        if fx.active_effect_id.get().is_some() {
            let controls_json = serde_json::json!({ name: value });
            leptos::task::spawn_local(async move {
                let _ = api::update_controls(&controls_json).await;
            });
        }
    });

    view! {
        <div class="flex flex-col h-full -m-6 animate-fade-in">
            // Fixed header — title + search + categories
            <div class="shrink-0 px-6 pt-6 pb-4 space-y-4 bg-layer-0 z-10">
                // Title row
                <div class="flex items-baseline justify-between">
                    <h1 class="text-lg font-medium text-fg">"Effects"</h1>
                    <span class="text-[11px] font-mono text-fg-dim tabular-nums">
                        {move || effect_count.get()} " effects"
                    </span>
                </div>

                // Search bar
                <div class="relative">
                    <svg class="absolute left-3.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-fg-dim pointer-events-none" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <circle cx="11" cy="11" r="8"/>
                        <path d="m21 21-4.3-4.3"/>
                    </svg>
                    <input
                        type="text"
                        placeholder="Search effects..."
                        class="w-full bg-layer-2/60 border border-white/[0.04] rounded-lg pl-9 pr-10 py-2 text-sm text-fg
                               placeholder-fg-dim focus:outline-none focus:border-electric-purple/20
                               focus:shadow-[0_0_0_1px_rgba(225,53,255,0.1),0_0_20px_rgba(225,53,255,0.06)]
                               transition-all duration-300"
                        prop:value=move || search.get()
                        on:input=move |ev| {
                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                            if let Some(el) = target {
                                set_search.set(el.value());
                            }
                        }
                    />
                    <kbd class="absolute right-3 top-1/2 -translate-y-1/2 text-[9px] font-mono text-fg-dim bg-white/[0.03] px-1.5 py-0.5 rounded border border-white/[0.03]">"/"</kbd>
                </div>

                // Category filter bar — each chip tinted with its category color
                <div class="flex gap-1.5 flex-wrap">
                    {CATEGORIES.iter().map(|cat| {
                        let cat = cat.to_string();
                        let cat_clone = cat.clone();
                        let rgb = if cat == "all" { "225, 53, 255" } else { category_accent_rgb(&cat) }.to_string();
                        let is_active = {
                            let cat = cat.clone();
                            Memo::new(move |_| category_filter.get() == cat)
                        };
                        let active_style = format!(
                            "background: rgba({rgb}, 0.15); color: rgb({rgb}); border-color: rgba({rgb}, 0.3); box-shadow: 0 0 12px rgba({rgb}, 0.2), inset 0 0 8px rgba({rgb}, 0.05)"
                        );
                        let inactive_style = format!(
                            "color: rgba({rgb}, 0.6); border-color: rgba({rgb}, 0.1); background: rgba({rgb}, 0.03)"
                        );
                        view! {
                            <button
                                class="px-2.5 py-1 rounded-full text-xs font-medium capitalize border chip-interactive"
                                style=move || if is_active.get() { active_style.clone() } else { inactive_style.clone() }
                                on:click=move |_| set_category_filter.set(cat_clone.clone())
                            >
                                {cat.clone()}
                            </button>
                        }
                    }).collect_view()}
                </div>
            </div>

            // Scrollable content: grid + pinned detail panel
            <div class="flex-1 overflow-y-auto px-6 pb-6">
                <div class="flex gap-5 items-start">
                    // Effect grid
                    <div class="flex-1 min-w-0">
                        <Suspense fallback=move || view! { <LoadingSkeleton /> }>
                            {move || {
                                let effects = filtered_effects.get();
                                if effects.is_empty() {
                                    view! {
                                        <div class="text-center py-20">
                                            <div class="text-fg-dim text-sm">"No effects found"</div>
                                            <div class="text-fg-dim/50 text-xs mt-1">"Try a different search or category"</div>
                                        </div>
                                    }.into_any()
                                } else {
                                    let grid_class = if has_active.get() {
                                        "grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-3"
                                    } else {
                                        "grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-4"
                                    };
                                    view! {
                                        <div class=grid_class>
                                            {effects.into_iter().enumerate().map(|(i, effect)| {
                                                let effect_id = effect.id.clone();
                                                let is_active = Signal::derive(move || {
                                                    fx.active_effect_id.get().as_deref() == Some(&effect_id)
                                                });
                                                view! {
                                                    <EffectCard
                                                        effect=effect
                                                        is_active=is_active
                                                        on_apply=on_apply
                                                        index=i
                                                    />
                                                }
                                            }).collect_view()}
                                        </div>
                                    }.into_any()
                                }
                            }}
                        </Suspense>
                    </div>

                    // Detail panel — sticky, scrolls with cards until bottom pins
                    {move || {
                        fx.active_effect_id.get().map(|_| {
                            let rgb = accent_rgb.get();
                            let dot_style = format!("background: rgb({}); box-shadow: 0 0 8px rgba({}, 0.6)", rgb, rgb);
                            let controls_accent = format!("border-top: 2px solid rgba({}, 0.15)", rgb);
                            view! {
                                <aside
                                    class="w-[420px] shrink-0 sticky top-0 self-start space-y-3 animate-slide-in-right scrollbar-none will-change-transform"
                                    style="max-height: calc(100vh - 10rem); overflow-y: auto"
                                >
                                    // Active effect name with category-colored dot
                                    {move || fx.active_effect_name.get().map(|name| {
                                        let dot_s = dot_style.clone();
                                        view! {
                                            <div class="flex items-center gap-2.5 px-1">
                                                <div class="w-2.5 h-2.5 rounded-full dot-alive shrink-0" style=dot_s />
                                                <span class="text-base font-medium text-fg">{name}</span>
                                            </div>
                                        }
                                    })}

                                    // Live preview — no border, black bleeds to edge
                                    <div class="rounded-xl bg-black overflow-hidden shadow-[0_4px_24px_rgba(0,0,0,0.3)]">

                                        <CanvasPreview
                                            frame=canvas_frame
                                            fps=ws_fps
                                            show_fps=true
                                        />
                                    </div>

                                    // Controls panel with category accent line
                                    <div
                                        class="rounded-xl bg-layer-1 border border-white/[0.06] p-5
                                               shadow-[0_2px_12px_rgba(0,0,0,0.2)]"
                                        style=controls_accent.clone()
                                    >
                                        <div class="flex items-center gap-2 mb-4">
                                            <svg class="w-4 h-4 text-fg-dim" viewBox="0 0 24 24" fill="none"
                                                 stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                <circle cx="12" cy="12" r="3" />
                                                <path d="M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42" />
                                            </svg>
                                            <h3 class="text-xs font-mono uppercase tracking-[0.12em] text-fg-dim">
                                                "Controls"
                                            </h3>
                                        </div>
                                        <ControlPanel
                                            controls=controls
                                            control_values=control_values
                                            accent_rgb=accent_rgb
                                            on_change=on_control_change
                                        />
                                    </div>
                                </aside>
                            }
                        })
                    }}
                </div>
            </div>
        </div>
    }
}

/// Loading skeleton for the effects grid.
#[component]
fn LoadingSkeleton() -> impl IntoView {
    view! {
        <div class="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-4">
            {(0..12).map(|_| {
                view! {
                    <div class="rounded-2xl border border-white/[0.03] bg-layer-2/40 px-4 py-4 animate-pulse space-y-3">
                        <div class="flex justify-between">
                            <div class="h-4 w-28 bg-white/[0.04] rounded" />
                            <div class="h-4 w-14 bg-white/[0.04] rounded-full" />
                        </div>
                        <div class="space-y-1.5">
                            <div class="h-3 w-full bg-white/[0.02] rounded" />
                            <div class="h-3 w-3/4 bg-white/[0.02] rounded" />
                        </div>
                        <div class="flex justify-between pt-1 border-t border-white/[0.02]">
                            <div class="h-2.5 w-16 bg-white/[0.02] rounded" />
                            <div class="h-2.5 w-12 bg-white/[0.02] rounded" />
                        </div>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}
