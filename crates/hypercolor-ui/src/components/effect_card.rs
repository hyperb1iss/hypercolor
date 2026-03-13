//! Effect card — cinematic card with category accent, favorite heart, capability badges,
//! hover glow, and active state.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::EffectSummary;
use crate::icons::*;
use crate::style_utils::category_style;

/// Source type label.
fn source_label(source: &str) -> &'static str {
    match source {
        "native" => "Native",
        "html" => "HTML",
        "shader" => "Shader",
        _ => "Other",
    }
}

fn source_badge_classes(source: &str) -> &'static str {
    match source {
        "native" => {
            "bg-electric-purple/10 text-electric-purple border border-electric-purple/15 \
             shadow-[0_0_18px_rgba(225,53,255,0.12)]"
        }
        "html" => "bg-neon-cyan/10 text-neon-cyan border border-neon-cyan/10",
        "shader" => "bg-info-blue/10 text-info-blue border border-info-blue/10",
        _ => "bg-surface-overlay/30 text-fg-tertiary/60 border border-edge-subtle",
    }
}

/// Cinematic effect card for the browse grid.
#[component]
pub fn EffectCard(
    effect: EffectSummary,
    #[prop(into)] is_active: Signal<bool>,
    #[prop(into)] is_favorite: Signal<bool>,
    #[prop(into)] on_apply: Callback<String>,
    #[prop(into)] on_toggle_favorite: Callback<String>,
    /// Index for stagger animation (clamped to 12).
    #[prop(default = 0)]
    index: usize,
) -> impl IntoView {
    let name = effect.name.clone();
    let description = effect.description.clone();
    let author = effect.author.clone();
    let category = effect.category.clone();
    let mut tags = effect.tags.clone();
    let runnable = effect.runnable;
    let audio_reactive = effect.audio_reactive;
    let source = effect.source.clone();
    let is_native = source == "native";
    if is_native && !tags.iter().any(|tag| tag.eq_ignore_ascii_case("native")) {
        tags.insert(0, "native".to_string());
    }

    let (badge_class, accent_rgb) = category_style(&category);
    let badge_class = badge_class.to_string();
    let accent_rgb = accent_rgb.to_string();

    // Category-colored top accent gradient
    let accent_gradient = format!(
        "background: linear-gradient(180deg, rgba({}, 0.06) 0%, transparent 40%)",
        accent_rgb
    );

    let click_id = effect.id.clone();
    let fav_id = effect.id.clone();
    let stagger = (index.min(12) + 1).to_string();
    let source_tag = source_label(&source).to_string();
    let source_badge_class = source_badge_classes(&source).to_string();

    view! {
        <div
            class=move || {
                let base = "relative rounded-2xl border text-left w-full group overflow-hidden \
                            card-hover animate-fade-in-up flex flex-col content-auto-card";
                let state = if is_active.get() {
                    "border-accent-muted bg-surface-overlay animate-breathe"
                } else if !runnable {
                    "border-edge-subtle bg-surface-overlay/40 opacity-30 cursor-not-allowed"
                } else {
                    "border-edge-subtle bg-surface-overlay/80 hover:border-edge-default"
                };
                format!("{base} {state} stagger-{}", stagger)
            }
            style:--glow-rgb=accent_rgb.clone()
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

            // Favorite heart button — top right, floats above content
            <button
                class="absolute top-3 right-3 z-10 p-1.5 rounded-full transition-all duration-200 \
                       hover:bg-surface-hover/40 hover:scale-110 active:scale-95"
                on:click={
                    let fav_id = fav_id.clone();
                    move |ev: web_sys::MouseEvent| {
                        ev.stop_propagation();
                        on_toggle_favorite.run(fav_id.clone());
                    }
                }
            >
                {move || {
                    let fav = is_favorite.get();
                    let (span_class, icon_style) = if fav {
                        (
                            "text-coral",
                            "fill: currentColor; filter: drop-shadow(0 0 8px rgba(255,106,193,0.6)); transition: color 0.2s, filter 0.3s",
                        )
                    } else {
                        (
                            "text-fg-tertiary/30 hover:text-fg-tertiary/60",
                            "transition: color 0.2s, filter 0.2s",
                        )
                    };
                    view! {
                        <span class=span_class style="transition: color 0.2s, filter 0.2s">
                            <Icon icon=LuHeart width="14px" height="14px" style=icon_style />
                        </span>
                    }
                }}
            </button>

            // Clickable card body
            <button
                class="relative flex flex-col flex-1 px-4 pt-4 pb-3 text-left"
                disabled=!runnable
                on:click=move |_| {
                    if runnable {
                        on_apply.run(click_id.clone());
                    }
                }
            >
                // Header: name + category badge
                <div class="flex items-start justify-between gap-3 pr-6 mb-2">
                    <h3 class="text-[15px] font-medium text-fg-primary group-hover:text-fg-primary line-clamp-2 transition-colors duration-200 leading-snug">
                        {name}
                    </h3>
                    <span class=format!("shrink-0 text-[9px] font-mono tracking-wide px-2 py-0.5 rounded-full capitalize {badge_class}")>
                        {category.clone()}
                    </span>
                </div>

                // Description
                <p class="text-xs text-fg-secondary/80 line-clamp-2 leading-relaxed mb-3">
                    {description}
                </p>

                // Capability badges row
                <div class="flex items-center gap-1.5 mb-3 flex-wrap">
                    // Audio reactive badge
                    {audio_reactive.then(|| view! {
                        <span class="inline-flex items-center gap-1 text-[9px] font-mono px-1.5 py-0.5 rounded \
                                     bg-coral/8 text-coral/80 border border-coral/10">
                            <Icon icon=LuAudioLines width="10px" height="10px" />
                            "Audio"
                        </span>
                    })}

                    // Source type badge
                    {(!is_native).then(|| view! {
                        <span class=format!(
                            "inline-flex items-center gap-1 text-[9px] font-mono px-1.5 py-0.5 rounded {}",
                            source_badge_class
                        )>
                            {if source == "html" {
                                view! { <Icon icon=LuGlobe width="10px" height="10px" /> }.into_any()
                            } else {
                                view! { <Icon icon=LuCode width="10px" height="10px" /> }.into_any()
                            }}
                            {source_tag}
                        </span>
                    })}
                </div>

                // Footer: author + tags
                <div class="flex items-center justify-between gap-2 pt-2 mt-auto border-t border-edge-subtle">
                    <span class="text-[10px] font-mono text-fg-tertiary truncate">{author}</span>
                    <div class="flex gap-1.5 overflow-hidden">
                        {tags.into_iter().take(3).map(|tag| {
                            view! {
                                <span class="text-[9px] font-mono text-fg-tertiary/70 bg-surface-overlay/30 px-1.5 py-0.5 rounded whitespace-nowrap">
                                    {tag}
                                </span>
                            }
                        }).collect_view()}
                    </div>
                </div>
            </button>
        </div>
    }
}
