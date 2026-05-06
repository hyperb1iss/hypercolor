//! Effect card — cinematic tile with curated or live-captured background
//! artwork, harmonized palette accents, and a single clean metadata row.
//!
//! Background resolution cascades (highest to lowest priority): a curated
//! screenshot served by the daemon at
//! `/api/v1/effects/screenshots/<slug>/default.webp`, then an opportunistic
//! thumbnail captured client-side from the live canvas and cached in
//! localStorage, then a category-coloured radial gradient. The curated image
//! paints on top when it loads; otherwise its `onerror` hides the element and
//! the lower layers remain visible.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::EffectSummary;
use crate::color;
use crate::icons::*;
use crate::style_utils::category_style;
use crate::thumbnails::{Thumbnail, ThumbnailStore};

/// Human label for the `source` enum ("native" → "Native", etc.).
fn source_label(source: &str) -> &'static str {
    match source {
        "native" => "Native",
        "html" => "HTML",
        "shader" => "Shader",
        _ => "Other",
    }
}

/// Kebab-case slug for an effect name. Mirrors the capture tool's slugify so
/// the UI and `effects/screenshots/curated/<slug>/` stay aligned — used to
/// build the curated screenshot URL.
fn slugify(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut prev_dash = true;
    for ch in value.chars() {
        let mapped = ch.to_ascii_lowercase();
        if mapped.is_ascii_alphanumeric() {
            out.push(mapped);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod slug_tests {
    use super::slugify;

    #[test]
    fn slugify_converts_names() {
        assert_eq!(slugify("Color Wave"), "color-wave");
        assert_eq!(slugify("ADHD Hyperfocus"), "adhd-hyperfocus");
        assert_eq!(slugify("Neon City"), "neon-city");
        assert_eq!(slugify("  Spaced  Out  "), "spaced-out");
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
    let category = effect.category.clone();
    let runnable = effect.runnable;
    let audio_reactive = effect.audio_reactive;
    let source = effect.source.clone();
    let is_calibration = effect.name.eq_ignore_ascii_case("Calibration")
        || effect
            .tags
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case("calibration"));

    let (_, fallback_rgb) = category_style(&category);
    let fallback_rgb = fallback_rgb.to_string();

    // Per-card reactive thumbnail — the store is a context, so every card
    // hits the same HashMap but only this card's derived signal reacts when
    // its own key updates.
    let thumb_store = use_context::<ThumbnailStore>();
    let thumb_id = effect.id.clone();
    let thumb_version = effect.version.clone();
    let thumbnail: Signal<Option<Thumbnail>> =
        Signal::derive(move || thumb_store.and_then(|store| store.get(&thumb_id, &thumb_version)));

    // Accent color drives the glow ring and metadata pill tints. Prefer the
    // captured palette's primary; fall back to category accent otherwise.
    // `Signal::derive` is `Copy`, so this can be freely reused across
    // multiple `move ||` style closures.
    let accent_rgb: Signal<String> = {
        let fallback = fallback_rgb.clone();
        Signal::derive(move || {
            thumbnail
                .get()
                .map(|t| t.palette.primary)
                .unwrap_or_else(|| fallback.clone())
        })
    };

    // Tinted text colors derived from the palette primary. Same hue as the
    // accent but locked to a readable lightness band so titles/descriptions
    // feel like they belong to this specific card rather than being generic
    // white. Re-computes only when the accent changes (Memo caches the String).
    let title_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.86, 0.65));
    let body_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.78, 0.22));
    let meta_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.68, 0.65));

    let click_id = effect.id.clone();
    let fav_id = effect.id.clone();
    let stagger = (index.min(12) + 1).to_string();
    let source_label_text = source_label(&source);
    let show_source_icon = source != "native";
    let is_html = source == "html";
    let curated_url = format!(
        "/api/v1/effects/screenshots/{}/default.webp",
        slugify(&effect.name)
    );
    let (curated_hidden, set_curated_hidden) = signal(false);

    view! {
        <div
            class=move || {
                let base = "relative rounded-xl border text-left w-full group overflow-hidden \
                            card-hover animate-fade-in-up aspect-[4/3] effect-card content-auto-card";
                let state = if is_active.get() {
                    "border-electric-purple/50"
                } else if !runnable {
                    "border-edge-subtle opacity-30 cursor-not-allowed"
                } else {
                    "border-edge-subtle hover:border-edge-default"
                };
                format!("{base} {state} stagger-{}", stagger)
            }
            style:--glow-rgb=move || accent_rgb.get()
        >
            // ── Background layers ────────────────────────────────────────
            // Paint order (bottom to top): category gradient → opportunistic
            // thumbnail div (when cached) → curated <img> (when reachable).
            // `onerror` on the curated image hides it so a missing screenshot
            // falls back through the stack naturally.
            {
                let fallback = fallback_rgb.clone();
                move || thumbnail.get().map_or_else(
                    || {
                        let bg = format!(
                            "background: \
                             radial-gradient(ellipse at 30% 25%, rgba({fb}, 0.28) 0%, transparent 55%), \
                             radial-gradient(ellipse at 75% 85%, rgba({fb}, 0.15) 0%, transparent 60%), \
                             linear-gradient(135deg, rgba(18, 14, 28, 1) 0%, rgba(10, 8, 18, 1) 100%)",
                            fb = fallback,
                        );
                        view! { <div class="absolute inset-0 pointer-events-none" style=bg /> }
                            .into_any()
                    },
                    |thumb| {
                        let bg = format!(
                            "background-image: url({}); background-size: cover; background-position: center",
                            thumb.data_url
                        );
                        view! {
                            <div
                                class="absolute inset-0 pointer-events-none scale-[1.02] \
                                       transition-transform duration-500 group-hover:scale-[1.06]"
                                style=bg
                            />
                        }.into_any()
                    },
                )
            }
            // `loading="lazy"` on absolute-positioned images breaks Chrome's
            // visibility math — cards below the first row never trigger a load.
            // `decoding="async"` still lets the browser off-thread decode.
            <img
                class=move || {
                    let base = "absolute inset-0 w-full h-full object-cover pointer-events-none \
                                scale-[1.02] transition-transform duration-500 group-hover:scale-[1.06]";
                    if curated_hidden.get() { "hidden" } else { base }
                }
                src=curated_url
                alt=""
                decoding="async"
                fetchpriority="low"
                on:error=move |_| set_curated_hidden.set(true)
            />


            // ── Scrim ────────────────────────────────────────────────────
            // Bottom-up darken gradient so the text area has legibility.
            <div
                class="absolute inset-0 pointer-events-none"
                style="background: linear-gradient(180deg, \
                       rgba(0, 0, 0, 0.15) 0%, \
                       rgba(0, 0, 0, 0.05) 30%, \
                       rgba(0, 0, 0, 0.72) 65%, \
                       rgba(0, 0, 0, 0.92) 100%)"
            />

            // ── Accent strip ────────────────────────────────────────────
            <div
                class="absolute top-0 left-0 right-0 h-[2px] z-[1]"
                style:background=move || format!(
                    "linear-gradient(90deg, rgba({a}, 0.5), rgba({a}, 0.08))",
                    a = accent_rgb.get()
                )
                style:box-shadow=move || format!(
                    "0 1px 8px rgba({}, 0.2)", accent_rgb.get()
                )
            />

            // ── Active-state accents ─────────────────────────────────────
            {move || is_active.get().then(|| view! {
                <div
                    class="absolute inset-0 rounded-xl pointer-events-none"
                    style="box-shadow: inset 0 0 0 1px rgba(225, 53, 255, 0.35), \
                           inset 0 1px 0 rgba(225, 53, 255, 0.25)"
                />
                <div class="absolute top-0 left-1/2 -translate-x-1/2 w-20 h-[2px] rounded-full bg-electric-purple/70 blur-[1px]" />
            })}

            // ── Favorite heart (top-right, floats above everything) ──────
            <button
                class="absolute top-2.5 right-2.5 z-20 p-1.5 rounded-full \
                       bg-black/30 backdrop-blur-sm transition-all duration-200 \
                       hover:bg-black/50 hover:scale-110 active:scale-95"
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
                            "fill: currentColor; filter: drop-shadow(0 0 6px rgba(255,106,193,0.7))",
                        )
                    } else {
                        ("text-white/70 hover:text-white", "")
                    };
                    view! {
                        <span class=span_class>
                            <Icon icon=LuHeart width="14px" height="14px" style=icon_style />
                        </span>
                    }
                }}
            </button>

            // ── Content overlay (clickable, fills the card) ──────────────
            <button
                class="absolute inset-0 z-10 flex flex-col justify-end px-4 pb-3.5 pt-4 text-left"
                disabled=!runnable
                on:click=move |_| {
                    if runnable {
                        on_apply.run(click_id.clone());
                    }
                }
            >
                // Title — tinted with palette primary, pushed near-white for legibility
                <h3
                    class="text-[15px] font-semibold line-clamp-2 leading-tight \
                           mb-1 drop-shadow-[0_2px_8px_rgba(0,0,0,0.85)]"
                    style:color=move || format!("rgb({})", title_tint.get())
                >
                    {name}
                </h3>

                // Description — softer tint so the title stays dominant
                <p
                    class="text-[11px] line-clamp-2 leading-relaxed mb-2.5 \
                           drop-shadow-[0_1px_4px_rgba(0,0,0,0.85)]"
                    style:color=move || format!("rgba({}, 0.88)", body_tint.get())
                >
                    {description}
                </p>

                // Single meta row — category on the left, source/audio icons on the right
                <div class="flex items-center justify-between gap-2">
                    // Category badge with palette-tinted dot
                    <div class="flex items-center gap-1.5 min-w-0">
                        <div
                            class="w-1.5 h-1.5 rounded-full shrink-0"
                            style:background=move || format!("rgb({})", accent_rgb.get())
                            style:box-shadow=move || format!("0 0 6px rgba({}, 0.7)", accent_rgb.get())
                        />
                        <span
                            class="text-[10px] font-mono uppercase tracking-wider capitalize truncate drop-shadow-[0_1px_3px_rgba(0,0,0,0.85)]"
                            style:color=move || format!("rgb({})", meta_tint.get())
                        >
                            {category.clone()}
                        </span>
                    </div>

                    // Right-side icon cluster: source + audio-reactive
                    <div class="flex items-center gap-1.5 shrink-0">
                        {is_calibration.then(|| view! {
                            <span
                                class="inline-flex items-center gap-1 text-[9px] font-mono px-1.5 py-0.5 rounded-full bg-neon-cyan/14 text-neon-cyan backdrop-blur-sm"
                                title="Layout setup and calibration tool"
                            >
                                <Icon icon=LuRadar width="11px" height="11px" />
                                <span>"Setup"</span>
                            </span>
                        })}
                        {show_source_icon.then(|| {
                            let icon_view = if is_html {
                                view! { <Icon icon=LuGlobe width="11px" height="11px" /> }.into_any()
                            } else {
                                view! { <Icon icon=LuCode width="11px" height="11px" /> }.into_any()
                            };
                            view! {
                                <span
                                    class="inline-flex items-center gap-1 text-[9px] font-mono \
                                           px-1.5 py-0.5 rounded-full bg-white/5 backdrop-blur-sm"
                                    style:color=move || format!("rgba({}, 0.85)", meta_tint.get())
                                    title=source_label_text
                                >
                                    {icon_view}
                                </span>
                            }
                        })}
                        {audio_reactive.then(|| view! {
                            <span
                                class="inline-flex items-center text-coral/90 \
                                       px-1.5 py-0.5 rounded-full bg-coral/15 backdrop-blur-sm"
                                title="Audio-reactive"
                            >
                                <Icon icon=LuAudioLines width="11px" height="11px" />
                            </span>
                        })}
                    </div>
                </div>
            </button>
        </div>
    }
}
