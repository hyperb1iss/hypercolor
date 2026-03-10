//! Effect card — cinematic card with stronger composition, live status, and category accenting.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::EffectSummary;
use crate::icons::*;

fn category_style(category: &str) -> (&'static str, &'static str) {
    match category {
        "ambient" => ("bg-neon-cyan/10 text-neon-cyan", "128, 255, 234"),
        "audio" => ("bg-coral/10 text-coral", "255, 106, 193"),
        "fun" => ("bg-electric-purple/10 text-electric-purple", "225, 53, 255"),
        "gaming" => ("bg-electric-purple/10 text-electric-purple", "225, 53, 255"),
        "particle" => ("bg-info-blue/10 text-info-blue", "130, 170, 255"),
        "reactive" => (
            "bg-electric-yellow/10 text-electric-yellow",
            "241, 250, 140",
        ),
        "generative" => ("bg-success-green/10 text-success-green", "80, 250, 123"),
        "interactive" => ("bg-info-blue/10 text-info-blue", "130, 170, 255"),
        "productivity" => ("bg-pink-soft/10 text-pink-soft", "255, 153, 255"),
        "utility" => ("bg-fg-tertiary/10 text-fg-tertiary", "139, 133, 160"),
        _ => ("bg-surface-overlay/50 text-fg-tertiary", "139, 133, 160"),
    }
}

fn source_label(source: &str) -> &'static str {
    match source {
        "native" => "Native",
        "html" => "HTML",
        "shader" => "Shader",
        _ => "Other",
    }
}

fn source_icon(source: &str) -> icondata_core::Icon {
    match source {
        "native" => LuDiamond,
        "html" => LuGlobe,
        "shader" => LuCode,
        _ => LuCircleDot,
    }
}

fn title_case_slug(value: &str) -> String {
    value
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            format!("{}{}", first.to_uppercase(), chars.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[component]
pub fn EffectCard(
    effect: EffectSummary,
    #[prop(into)] is_active: Signal<bool>,
    #[prop(into)] is_favorite: Signal<bool>,
    #[prop(into)] on_apply: Callback<String>,
    #[prop(into)] on_toggle_favorite: Callback<String>,
    #[prop(default = 0)] index: usize,
) -> impl IntoView {
    let name = effect.name.clone();
    let description = effect.description.clone();
    let author = effect.author.clone();
    let category = effect.category.clone();
    let tags = effect.tags.clone();
    let runnable = effect.runnable;
    let audio_reactive = effect.audio_reactive;
    let source = effect.source.clone();

    let (badge_class, accent_rgb) = category_style(&category);
    let badge_class = badge_class.to_string();
    let accent_rgb = accent_rgb.to_string();
    let accent_gradient = format!(
        "background: linear-gradient(180deg, rgba({}, 0.08) 0%, transparent 40%)",
        accent_rgb
    );

    let click_id = effect.id.clone();
    let fav_id = effect.id.clone();
    let stagger = (index.min(12) + 1).to_string();
    let source_tag = source_label(&source).to_string();
    let category_label = title_case_slug(&category);
    let source_icon = source_icon(&source);

    let source_tile = format!(
        "background: linear-gradient(180deg, rgba({accent_rgb}, 0.18), rgba({accent_rgb}, 0.06)); \
         border: 1px solid rgba({accent_rgb}, 0.18); \
         box-shadow: inset 0 1px 0 rgba(255,255,255,0.05);"
    );
    let top_beam = format!(
        "background: linear-gradient(90deg, transparent, rgba({accent_rgb}, 0.75), transparent)"
    );

    view! {
        <div
            class=move || {
                let base = "relative rounded-[1.35rem] border text-left w-full group overflow-hidden \
                            card-hover animate-fade-in-up flex flex-col content-auto-card";
                let state = if is_active.get() {
                    "border-accent-muted bg-surface-overlay animate-breathe"
                } else if !runnable {
                    "border-edge-subtle bg-surface-overlay/40 opacity-40 cursor-not-allowed"
                } else {
                    "border-edge-subtle bg-surface-overlay/85 hover:border-edge-default"
                };
                format!("{base} {state} stagger-{stagger}")
            }
            style:--glow-rgb=accent_rgb.clone()
        >
            <div class="absolute inset-0 pointer-events-none" style=accent_gradient />
            <div class="absolute top-0 left-6 right-6 h-px rounded-full opacity-80" style=top_beam />

            {move || is_active.get().then(|| view! {
                <div
                    class="absolute inset-0 pointer-events-none"
                    style="background: radial-gradient(ellipse at 50% -10%, rgba(225, 53, 255, 0.16) 0%, transparent 65%); \
                           box-shadow: inset 0 1px 0 rgba(225, 53, 255, 0.16)"
                />
            })}

            <button
                type="button"
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

            <button
                type="button"
                class="relative flex flex-col flex-1 px-4 pt-4 pb-4 text-left"
                disabled=!runnable
                on:click=move |_| {
                    if runnable {
                        on_apply.run(click_id.clone());
                    }
                }
            >
                <div class="flex items-start justify-between gap-3 pr-8 mb-4">
                    <div class="flex items-start gap-3 min-w-0">
                        <div
                            class="w-11 h-11 rounded-2xl inline-flex items-center justify-center shrink-0"
                            style=source_tile
                        >
                            <Icon icon=source_icon width="16px" height="16px" style="color: rgba(230, 237, 243, 0.92)" />
                        </div>
                        <div class="min-w-0">
                            <div class="flex flex-wrap items-center gap-2 text-[10px] font-mono uppercase tracking-[0.16em] text-fg-tertiary/62">
                                <span>{source_tag}</span>
                                {audio_reactive.then(|| view! {
                                    <span class="inline-flex items-center gap-1 text-coral/80">
                                        <Icon icon=LuAudioLines width="10px" height="10px" />
                                        "Reactive"
                                    </span>
                                })}
                            </div>
                            <h3 class="mt-1 text-[15px] font-medium text-fg-primary line-clamp-2 leading-snug">
                                {name}
                            </h3>
                        </div>
                    </div>
                    <span class=format!("shrink-0 text-[9px] font-mono tracking-wide px-2 py-0.5 rounded-full capitalize {badge_class}")>
                        {category_label}
                    </span>
                </div>

                <p class="text-sm text-fg-secondary/82 line-clamp-3 leading-relaxed mb-4 min-h-[4rem]">
                    {description}
                </p>

                <div class="flex flex-wrap items-center gap-1.5 mb-4">
                    {audio_reactive.then(|| view! {
                        <span class="inline-flex items-center gap-1 text-[9px] font-mono px-1.5 py-0.5 rounded border border-coral/14 bg-coral/8 text-coral/82">
                            <Icon icon=LuAudioLines width="10px" height="10px" />
                            "Audio"
                        </span>
                    })}
                    {tags.into_iter().take(2).map(|tag| {
                        view! {
                            <span class="text-[9px] font-mono text-fg-tertiary/72 bg-white/[0.03] border border-white/6 px-1.5 py-0.5 rounded whitespace-nowrap">
                                {tag}
                            </span>
                        }
                    }).collect_view()}
                </div>

                <div class="mt-auto pt-3 border-t border-edge-subtle flex items-center justify-between gap-3">
                    <span class="inline-flex items-center gap-1.5 text-[10px] font-mono text-fg-tertiary/78 truncate">
                        <Icon icon=LuUser width="11px" height="11px" />
                        <span class="truncate">{author}</span>
                    </span>
                    <span
                        class="text-[10px] font-mono uppercase tracking-[0.14em]"
                        style=move || {
                            if is_active.get() {
                                "color: rgba(128, 255, 234, 0.82)"
                            } else if runnable {
                                "color: rgba(225, 53, 255, 0.72)"
                            } else {
                                "color: rgba(139, 133, 160, 0.48)"
                            }
                        }
                    >
                        {move || {
                            if is_active.get() {
                                "Now live"
                            } else if runnable {
                                "Load effect"
                            } else {
                                "Unavailable"
                            }
                        }}
                    </span>
                </div>
            </button>
        </div>
    }
}
