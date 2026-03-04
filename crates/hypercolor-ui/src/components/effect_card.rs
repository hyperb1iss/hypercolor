//! Effect card — cinematic card with category accent, hover glow, and active state.

use leptos::prelude::*;

use crate::api::EffectSummary;

/// Category → (badge classes, accent hex for gradients).
fn category_style(category: &str) -> (&'static str, &'static str) {
    match category {
        "ambient" => ("bg-neon-cyan/10 text-neon-cyan", "128, 255, 234"),
        "audio" => ("bg-coral/10 text-coral", "255, 106, 193"),
        "gaming" => ("bg-electric-purple/10 text-electric-purple", "225, 53, 255"),
        "reactive" => ("bg-electric-yellow/10 text-electric-yellow", "241, 250, 140"),
        "generative" => ("bg-success-green/10 text-success-green", "80, 250, 123"),
        "interactive" => ("bg-info-blue/10 text-info-blue", "130, 170, 255"),
        "productivity" => ("bg-pink-soft/10 text-pink-soft", "255, 153, 255"),
        "utility" => ("bg-fg-muted/10 text-fg-muted", "139, 133, 160"),
        _ => ("bg-white/5 text-zinc-400", "139, 133, 160"),
    }
}

/// Cinematic effect card for the browse grid.
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

    let (badge_class, accent_rgb) = category_style(&category);
    let badge_class = badge_class.to_string();
    let accent_rgb = accent_rgb.to_string();

    // Category-colored top accent gradient
    let accent_gradient = format!(
        "background: linear-gradient(180deg, rgba({}, 0.06) 0%, transparent 40%)",
        accent_rgb
    );

    // Hover glow shadow
    let hover_glow = format!(
        "0 8px 32px rgba({}, 0.08), 0 0 1px rgba({}, 0.2)",
        accent_rgb, accent_rgb
    );

    let click_id = effect.id.clone();

    view! {
        <button
            class=move || {
                let base = "relative rounded-2xl border text-left w-full group overflow-hidden \
                            transition-all duration-200 ease-out animate-fade-in-up";
                let state = if is_active.get() {
                    "border-electric-purple/30 bg-layer-2 shadow-[0_0_30px_rgba(225,53,255,0.1)]"
                } else if !runnable {
                    "border-white/[0.03] bg-layer-2/40 opacity-30 cursor-not-allowed"
                } else {
                    "border-white/[0.05] bg-layer-2/80 hover:border-white/10"
                };
                format!("{base} {state}")
            }
            style:--hover-glow=hover_glow.clone()
            disabled=!runnable
            on:click=move |_| {
                if runnable {
                    on_apply.run(click_id.clone());
                }
            }
        >
            // Category accent gradient overlay
            <div class="absolute inset-0 pointer-events-none rounded-2xl" style=accent_gradient />

            // Active electric glow
            {move || is_active.get().then(|| view! {
                <div
                    class="absolute inset-0 rounded-2xl pointer-events-none"
                    style="background: radial-gradient(ellipse at 50% -20%, rgba(225, 53, 255, 0.15) 0%, transparent 65%); \
                           box-shadow: inset 0 1px 0 rgba(225, 53, 255, 0.2)"
                />
                <div class="absolute top-0 left-1/2 -translate-x-1/2 w-16 h-[2px] rounded-full bg-electric-purple/60 blur-[2px]" />
            })}

            <div class="relative px-4 py-4 space-y-2.5">
                // Header: name + category badge
                <div class="flex items-start justify-between gap-3">
                    <h3 class="text-sm font-medium text-zinc-200 group-hover:text-fg line-clamp-1 transition-colors duration-200 leading-snug">
                        {name}
                    </h3>
                    <span class=format!("shrink-0 text-[9px] font-mono tracking-wide px-2 py-0.5 rounded-full capitalize {badge_class}")>
                        {category.clone()}
                    </span>
                </div>

                // Description (two lines for richness)
                <p class="text-xs text-fg-muted/80 line-clamp-2 leading-relaxed min-h-[2.25rem]">
                    {description}
                </p>

                // Footer: author + tags
                <div class="flex items-center justify-between gap-2 pt-1 border-t border-white/[0.03]">
                    <span class="text-[10px] font-mono text-fg-dim truncate">{author}</span>
                    <div class="flex gap-1.5 overflow-hidden">
                        {tags.into_iter().take(2).map(|tag| {
                            view! {
                                <span class="text-[9px] font-mono text-fg-dim/70 bg-white/[0.03] px-1.5 py-0.5 rounded whitespace-nowrap">
                                    {tag}
                                </span>
                            }
                        }).collect_view()}
                    </div>
                </div>
            </div>
        </button>
    }
}
