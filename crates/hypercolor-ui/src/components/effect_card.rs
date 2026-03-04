//! Effect card — thumbnail card for the effect browser grid.

use leptos::prelude::*;

use crate::api::EffectSummary;

/// Category → accent color class mapping.
fn category_color(category: &str) -> &'static str {
    match category {
        "ambient" => "bg-neon-cyan/15 text-neon-cyan border-neon-cyan/30",
        "audio" => "bg-coral/15 text-coral border-coral/30",
        "gaming" => "bg-electric-purple/15 text-electric-purple border-electric-purple/30",
        "reactive" => "bg-electric-yellow/15 text-electric-yellow border-electric-yellow/30",
        "generative" => "bg-success-green/15 text-success-green border-success-green/30",
        "interactive" => "bg-coral/15 text-coral border-coral/30",
        "productivity" => "bg-neon-cyan/15 text-neon-cyan border-neon-cyan/30",
        "utility" => "bg-zinc-500/15 text-zinc-400 border-zinc-500/30",
        _ => "bg-zinc-500/15 text-zinc-400 border-zinc-500/30",
    }
}

/// A single effect card in the browse grid.
#[component]
pub fn EffectCard(
    effect: EffectSummary,
    #[prop(into)] is_active: Signal<bool>,
    #[prop(into)] on_apply: Callback<String>,
) -> impl IntoView {
    let name = effect.name.clone();
    let description = effect.description.clone();
    let author = effect.author.clone();
    let category = effect.category.clone();
    let tags = effect.tags.clone();
    let runnable = effect.runnable;
    let category_class = category_color(&category).to_string();

    let click_id = effect.id.clone();

    view! {
        <button
            class="group relative flex flex-col text-left rounded-xl border transition-all duration-200 p-4 w-full outline-none focus-visible:ring-1 focus-visible:ring-electric-purple/50"
            class=("border-electric-purple/60 bg-electric-purple/5 shadow-[0_0_20px_rgba(225,53,255,0.08)]", move || is_active.get())
            class=("border-white/5 bg-layer-2 hover:bg-layer-3 hover:border-white/10", move || !is_active.get())
            class=("opacity-40 cursor-not-allowed", !runnable)
            disabled=!runnable
            on:click=move |_| on_apply.run(click_id.clone())
        >
            // Header: name + category badge
            <div class="flex items-start justify-between gap-2 mb-2">
                <h3 class="text-sm font-medium text-zinc-100 leading-tight line-clamp-1 group-hover:text-white transition-colors">
                    {name.clone()}
                </h3>
                <span class={format!("shrink-0 text-[10px] font-medium px-1.5 py-0.5 rounded-full border {category_class}")}>
                    {category.clone()}
                </span>
            </div>

            // Description
            <p class="text-xs text-zinc-500 leading-relaxed line-clamp-2 mb-3 min-h-[2.5rem]">
                {description}
            </p>

            // Footer: author + tags
            <div class="flex items-center justify-between mt-auto">
                <span class="text-[10px] text-zinc-600 font-mono">{author}</span>
                <div class="flex gap-1">
                    {tags.into_iter().take(2).map(|tag| {
                        view! {
                            <span class="text-[9px] text-zinc-600 bg-white/[0.03] px-1.5 py-0.5 rounded">
                                {tag}
                            </span>
                        }
                    }).collect_view()}
                </div>
            </div>

            // Active glow indicator
            <div
                class="absolute inset-0 rounded-xl pointer-events-none transition-opacity duration-300"
                class=("opacity-100", move || is_active.get())
                class=("opacity-0", move || !is_active.get())
                style="background: radial-gradient(ellipse at top, rgba(225,53,255,0.04) 0%, transparent 70%)"
            />
        </button>
    }
}
