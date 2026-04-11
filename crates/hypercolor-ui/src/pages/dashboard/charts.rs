//! Dashboard chart panels — pipeline, distribution, throughput, favorites, resize hint.

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::EffectSummary;
use crate::app::EffectsContext;
use crate::color;
use crate::components::perf_charts::{DistributionBar, Sparkline, StackSegment, StackedBar};
use crate::icons::*;
use crate::style_utils::category_style;
use crate::thumbnails::{Thumbnail, ThumbnailStore};
use crate::ws::PerformanceMetrics;

// ── Pipeline breakdown ───────────────────────────────────────────────

#[component]
pub(super) fn PipelinePanel(
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
) -> impl IntoView {
    let segments = Memo::new(move |_| {
        let Some(m) = metrics.get() else {
            return Vec::<StackSegment>::new();
        };
        let s = &m.stages;
        vec![
            StackSegment {
                label: "Input",
                value: s.input_sampling_ms,
                color: "#80ffea",
            },
            StackSegment {
                label: "Producer",
                value: s.producer_rendering_ms,
                color: "#e135ff",
            },
            StackSegment {
                label: "Compose",
                value: s.composition_ms,
                color: "#ff6ac1",
            },
            StackSegment {
                label: "Sample",
                value: s.spatial_sampling_ms,
                color: "#ff99ff",
            },
            StackSegment {
                label: "Output",
                value: s.device_output_ms,
                color: "#f1fa8c",
            },
            StackSegment {
                label: "Post",
                value: s.preview_postprocess_ms,
                color: "#82aaff",
            },
            StackSegment {
                label: "Publish",
                value: s.event_bus_ms,
                color: "#50fa7b",
            },
            StackSegment {
                label: "Overhead",
                value: s.coordination_overhead_ms,
                color: "#808090",
            },
        ]
    });
    let total_label = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| {
                let s = &m.stages;
                let total = s.input_sampling_ms
                    + s.producer_rendering_ms
                    + s.composition_ms
                    + s.spatial_sampling_ms
                    + s.device_output_ms
                    + s.preview_postprocess_ms
                    + s.event_bus_ms
                    + s.coordination_overhead_ms;
                format!(
                    "Σ {total:.2} ms · budget {:.1} ms",
                    if m.fps.target > 0 {
                        1000.0 / f64::from(m.fps.target)
                    } else {
                        33.33
                    }
                )
            })
            .unwrap_or_else(|| "collecting".into())
    });

    view! {
        <div
            class="rounded-lg bg-surface-overlay/40 border border-transparent"
            style="border-top: 2px solid rgba(225, 53, 255, 0.25)"
        >
            <div class="px-4 py-2.5 flex items-center justify-between">
                <div class="flex items-center gap-2">
                    <Icon icon=LuLayers width="14px" height="14px" style="color: var(--color-electric-purple)" />
                    <h2 class="text-[13px] font-medium text-fg-secondary">"Pipeline Breakdown"</h2>
                </div>
                <div class="text-[10px] font-mono text-fg-tertiary/70">
                    {move || total_label.get()}
                </div>
            </div>
            <div class="p-4">
                <StackedBar
                    segments=Signal::derive(move || segments.get())
                    total_override=None
                    height=34
                />
            </div>
        </div>
    }
}

// ── Distribution panel (frame time percentiles) ─────────────────────

#[component]
pub(super) fn DistributionPanel(
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
) -> impl IntoView {
    let avg = Memo::new(move |_| metrics.get().map_or(0.0, |m| m.frame_time.avg_ms));
    let p95 = Memo::new(move |_| metrics.get().map_or(0.0, |m| m.frame_time.p95_ms));
    let p99 = Memo::new(move |_| metrics.get().map_or(0.0, |m| m.frame_time.p99_ms));
    let max = Memo::new(move |_| metrics.get().map_or(0.0, |m| m.frame_time.max_ms));
    let budget = Memo::new(move |_| {
        metrics.get().map_or(33.33, |m| {
            if m.fps.target > 0 {
                1000.0 / f64::from(m.fps.target)
            } else {
                33.33
            }
        })
    });

    view! {
        <div class="pt-1">
            <div class="flex items-center justify-between mb-3">
                <div class="flex items-center gap-2">
                    <Icon icon=LuGauge width="14px" height="14px" style="color: var(--color-coral)" />
                    <h2 class="text-[13px] font-medium text-fg-secondary">"Frame Time Distribution"</h2>
                </div>
                <div class="text-[10px] font-mono text-fg-tertiary/70">
                    {move || format!("budget {:.1} ms", budget.get())}
                </div>
            </div>
            <DistributionBar avg=avg p95=p95 p99=p99 max=max budget=budget />
        </div>
    }
}

// ── Throughput panel ─────────────────────────────────────────────────

#[component]
pub(super) fn ThroughputPanel(
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
    #[prop(into)] ws_bytes_series: Signal<Vec<f64>>,
) -> impl IntoView {
    let ws_bytes = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format_bytes_per_sec(m.websocket.bytes_sent_per_sec))
            .unwrap_or_else(|| "—".into())
    });
    let ws_clients = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| {
                format!(
                    "{} client{}",
                    m.websocket.client_count,
                    if m.websocket.client_count == 1 {
                        ""
                    } else {
                        "s"
                    }
                )
            })
            .unwrap_or_else(|| "metrics channel".into())
    });

    view! {
        <div
            class="rounded-lg bg-surface-overlay/40 border border-transparent"
            style="border-top: 2px solid rgba(241, 250, 140, 0.20)"
        >
            <div class="px-4 py-2.5 flex items-center justify-between">
                <div class="flex items-center gap-2">
                    <Icon icon=LuWifi width="14px" height="14px" style="color: var(--color-electric-yellow)" />
                    <h2 class="text-[13px] font-medium text-fg-secondary">"WebSocket Throughput"</h2>
                </div>
                <div class="text-[10px] font-mono text-fg-tertiary/70">
                    {move || ws_clients.get()}
                </div>
            </div>
            <div class="p-4 grid grid-cols-1 md:grid-cols-[auto_1fr] gap-4 items-center">
                <div class="flex flex-col">
                    <span class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary">"Bytes / sec"</span>
                    <span class="text-[22px] font-semibold tabular-nums text-electric-yellow mt-0.5">
                        {move || ws_bytes.get()}
                    </span>
                </div>
                <div class="h-14">
                    <Sparkline
                        values=ws_bytes_series
                        stroke="var(--color-electric-yellow)"
                    />
                </div>
            </div>
        </div>
    }
}

fn format_bytes_per_sec(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1_000_000.0 {
        format!("{:.2} MB/s", bytes_per_sec / 1_000_000.0)
    } else if bytes_per_sec >= 1_000.0 {
        format!("{:.1} KB/s", bytes_per_sec / 1_000.0)
    } else {
        format!("{bytes_per_sec:.0} B/s")
    }
}

// ── Favorites (cinematic) ────────────────────────────────────────────
//
// Each favorite renders as a 72px horizontal cinema tile — live thumbnail
// backdrop (or category-tinted radial fallback), palette-tinted title, a
// glowing accent stripe keyed to the extracted palette primary, and an
// equalizer-bar / "Now Playing" treatment when the effect is currently
// live. Mirrors the cinematic language of `EffectCard` but adapted to the
// vertical sidebar budget on the dashboard.

#[component]
pub(super) fn FavoritesPanel() -> impl IntoView {
    let fx = expect_context::<EffectsContext>();

    let favorites_count = Memo::new(move |_| fx.favorite_ids.get().len());

    let favorite_effects = Memo::new(move |_| {
        let fav_ids = fx.favorite_ids.get();
        fx.effects_index
            .get()
            .into_iter()
            .filter(|entry| fav_ids.contains(&entry.effect.id))
            .map(|entry| entry.effect)
            .collect::<Vec<_>>()
    });

    view! {
        <div
            class="relative rounded-xl glass-subtle flex flex-col flex-1 min-h-0 overflow-hidden"
            style="background: \
                   linear-gradient(180deg, \
                     rgba(255, 106, 193, 0.05) 0%, \
                     rgba(225, 53, 255, 0.02) 45%, \
                     transparent 100%), \
                   var(--glass-bg); \
                   border: 1px solid rgba(255, 106, 193, 0.16); \
                   box-shadow: 0 0 32px rgba(255, 106, 193, 0.06), \
                               inset 0 1px 0 rgba(255, 255, 255, 0.04)"
        >
            // Top-edge color strip: coral → purple → cyan.
            <div
                class="absolute top-0 left-0 right-0 h-[2px] pointer-events-none"
                style="background: linear-gradient(90deg, \
                         transparent 0%, \
                         rgba(255, 106, 193, 0.9) 18%, \
                         rgba(225, 53, 255, 0.95) 50%, \
                         rgba(128, 255, 234, 0.9) 82%, \
                         transparent 100%); \
                       filter: drop-shadow(0 0 6px rgba(225, 53, 255, 0.35))"
            />

            // Header
            <div class="shrink-0 px-4 pt-3 pb-2.5 flex items-center justify-between relative">
                <div class="flex items-center gap-2">
                    <div
                        class="relative flex items-center justify-center w-6 h-6 rounded-lg"
                        style="background: radial-gradient(circle at center, \
                                 rgba(255, 106, 193, 0.22) 0%, transparent 70%)"
                    >
                        <Icon
                            icon=LuHeart
                            width="13px"
                            height="13px"
                            style="color: var(--color-coral); \
                                   fill: currentColor; \
                                   filter: drop-shadow(0 0 6px rgba(255, 106, 193, 0.7))"
                        />
                    </div>
                    <h2
                        class="text-[12px] font-semibold tracking-wide"
                        style="background: linear-gradient(90deg, \
                                 #ff6ac1 0%, #e135ff 45%, #80ffea 100%); \
                               background-size: 200% 100%; \
                               -webkit-background-clip: text; \
                               background-clip: text; \
                               -webkit-text-fill-color: transparent; \
                               color: transparent; \
                               animation: shimmer 6s linear infinite"
                    >
                        "Favorites"
                    </h2>
                </div>
                <div
                    class="text-[10px] font-mono rounded-full px-2 py-[3px] tabular-nums"
                    style="color: var(--color-coral); \
                           background: rgba(255, 106, 193, 0.08); \
                           border: 1px solid rgba(255, 106, 193, 0.22); \
                           box-shadow: 0 0 8px rgba(255, 106, 193, 0.12), \
                                       inset 0 1px 0 rgba(255, 255, 255, 0.04)"
                >
                    {move || favorites_count.get().to_string()}
                </div>
            </div>

            // Scrollable list
            <div class="flex-1 overflow-y-auto min-h-0 px-2 pb-2 pt-0.5">
                {move || {
                    let effects = favorite_effects.get();
                    if effects.is_empty() {
                        view! { <FavoritesEmpty /> }.into_any()
                    } else {
                        view! {
                            <div class="flex flex-col gap-1.5">
                                {effects.into_iter().enumerate().map(|(i, effect)| {
                                    view! { <FavoriteCinemaCard effect=effect index=i /> }
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

/// Atmospheric empty state — pulsing dashed ring and inviting copy.
#[component]
fn FavoritesEmpty() -> impl IntoView {
    view! {
        <div class="flex flex-col items-center justify-center h-full py-10 text-center px-4 gap-3 animate-fade-in">
            <div class="relative w-16 h-16 flex items-center justify-center">
                <div
                    class="absolute inset-0 rounded-full animate-breathe"
                    style="--glow-rgb: 255, 106, 193; \
                           border: 1px dashed rgba(255, 106, 193, 0.4); \
                           background: radial-gradient(circle at center, \
                             rgba(255, 106, 193, 0.12) 0%, transparent 65%)"
                />
                <Icon
                    icon=LuHeart
                    width="22px"
                    height="22px"
                    style="color: var(--color-coral); \
                           opacity: 0.78; \
                           filter: drop-shadow(0 0 10px rgba(255, 106, 193, 0.55))"
                />
            </div>
            <div class="space-y-1">
                <p class="text-[12px] font-medium text-fg-secondary">"Nothing pinned yet"</p>
                <p class="text-[10px] text-fg-tertiary leading-relaxed">
                    "Tap the heart on any effect to save it here."
                </p>
            </div>
        </div>
    }
}

/// Cinematic horizontal favorite tile — thumbnail backdrop, palette-tinted
/// title, accent stripe, and active-state theatrics.
#[component]
fn FavoriteCinemaCard(effect: EffectSummary, index: usize) -> impl IntoView {
    let fx = expect_context::<EffectsContext>();
    let thumb_store = use_context::<ThumbnailStore>();

    let apply_id = effect.id.clone();
    let fav_id = effect.id.clone();
    let active_check_id = effect.id.clone();
    let thumb_id = effect.id.clone();
    let thumb_version = effect.version.clone();
    let name = effect.name.clone();
    let category = effect.category.clone();
    let audio_reactive = effect.audio_reactive;

    let (_, fallback_rgb) = category_style(&category);
    let fallback_rgb = fallback_rgb.to_string();

    // Per-card reactive thumbnail lookup.
    let thumbnail: Signal<Option<Thumbnail>> =
        Signal::derive(move || thumb_store.and_then(|store| store.get(&thumb_id, &thumb_version)));

    // Palette primary drives the accent stripe + title tint; secondary feeds
    // the fallback-gradient second radial so an un-captured card still looks
    // visually distinct from the category badge alone.
    let accent_rgb: Signal<String> = {
        let fallback = fallback_rgb.clone();
        Signal::derive(move || {
            thumbnail
                .get()
                .map(|t| t.palette.primary)
                .unwrap_or_else(|| fallback.clone())
        })
    };
    let secondary_rgb: Signal<String> = {
        let fallback = fallback_rgb.clone();
        Signal::derive(move || {
            thumbnail
                .get()
                .map(|t| t.palette.secondary)
                .unwrap_or_else(|| fallback.clone())
        })
    };

    let title_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.90, 0.55));
    let meta_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.74, 0.55));

    let is_active = Signal::derive(move || {
        fx.active_effect_id.get().as_deref() == Some(active_check_id.as_str())
    });
    let is_playing = fx.is_playing;
    let is_live = Signal::derive(move || is_active.get() && is_playing.get());

    let stagger = index.min(11) + 1;
    let fallback_for_bg = fallback_rgb.clone();
    let category_for_meta = category.clone();

    view! {
        <div
            class=format!("animate-fade-in-up stagger-{stagger}")
            style:--glow-rgb=move || accent_rgb.get()
        >
            <div
                class=move || {
                    let base = "relative w-full rounded-xl overflow-hidden card-hover border \
                                h-[72px] group";
                    if is_active.get() {
                        format!("{base} border-transparent animate-breathe")
                    } else {
                        format!("{base} border-edge-subtle/40")
                    }
                }
            >
                // ── Background: thumbnail when captured, category gradient otherwise.
                {move || thumbnail.get().map_or_else(
                    || {
                        let bg = format!(
                            "background: \
                               radial-gradient(ellipse at 12% 45%, rgba({fb}, 0.42) 0%, transparent 55%), \
                               radial-gradient(ellipse at 85% 70%, rgba({sc}, 0.26) 0%, transparent 60%), \
                               linear-gradient(120deg, rgba(18, 14, 28, 0.96) 0%, rgba(10, 8, 20, 0.96) 100%)",
                            fb = fallback_for_bg,
                            sc = secondary_rgb.get(),
                        );
                        view! {
                            <div class="absolute inset-0 pointer-events-none" style=bg />
                        }.into_any()
                    },
                    |thumb| {
                        let bg = format!(
                            "background-image: url({}); \
                             background-size: cover; \
                             background-position: center",
                            thumb.data_url
                        );
                        view! {
                            <div
                                class="absolute inset-0 pointer-events-none scale-[1.04] \
                                       transition-transform duration-500 ease-out \
                                       group-hover:scale-[1.1]"
                                style=bg
                            />
                        }.into_any()
                    },
                )}

                // ── Legibility scrim: heavier on the left where text lives.
                <div
                    class="absolute inset-0 pointer-events-none"
                    style="background: linear-gradient(90deg, \
                             rgba(0, 0, 0, 0.80) 0%, \
                             rgba(0, 0, 0, 0.58) 38%, \
                             rgba(0, 0, 0, 0.22) 78%, \
                             rgba(0, 0, 0, 0.08) 100%)"
                />

                // ── Palette-tinted accent stripe on the left edge.
                <div
                    class="absolute top-0 bottom-0 left-0 w-[3px] pointer-events-none transition-all duration-300"
                    style=move || {
                        let rgb = accent_rgb.get();
                        let glow = if is_active.get() {
                            format!("0 0 14px rgba({rgb}, 0.85), 0 0 2px rgba({rgb}, 1)")
                        } else {
                            format!("0 0 6px rgba({rgb}, 0.38)")
                        };
                        format!(
                            "background: linear-gradient(180deg, \
                                 rgba({rgb}, 0.18) 0%, \
                                 rgba({rgb}, 1) 50%, \
                                 rgba({rgb}, 0.18) 100%); \
                             box-shadow: {glow}"
                        )
                    }
                />

                // ── Active inset glow ring — colour-matches the palette.
                {move || is_active.get().then(|| {
                    let rgb = accent_rgb.get();
                    view! {
                        <div
                            class="absolute inset-0 rounded-xl pointer-events-none"
                            style=format!(
                                "box-shadow: inset 0 0 0 1px rgba({rgb}, 0.45), \
                                             inset 0 1px 0 rgba({rgb}, 0.3)"
                            )
                        />
                    }
                })}

                // ── Main click target.
                <button
                    class="absolute inset-0 flex items-center gap-2.5 pl-4 pr-10 text-left btn-press"
                    on:click={
                        let apply_id = apply_id.clone();
                        move |_| fx.apply_effect(apply_id.clone())
                    }
                >
                    // Equalizer bars only when actively playing this favorite.
                    {move || is_live.get().then(|| {
                        let rgb = accent_rgb.get();
                        let bar = |delay: &str, rgb: &str| {
                            let style = format!(
                                "background: rgb({rgb}); \
                                 box-shadow: 0 0 6px rgba({rgb}, 0.8); \
                                 animation-delay: {delay}"
                            );
                            view! { <div class="w-[2px] h-full rounded-full animate-eq-bar" style=style /> }
                        };
                        view! {
                            <div class="flex items-end gap-[2px] h-5 shrink-0">
                                {bar("0ms", &rgb)}
                                {bar("140ms", &rgb)}
                                {bar("280ms", &rgb)}
                                {bar("110ms", &rgb)}
                            </div>
                        }
                    })}

                    <div class="flex-1 min-w-0 flex flex-col gap-[3px]">
                        // Ribbon: "Now Playing" when live, category label otherwise.
                        {move || {
                            if is_live.get() {
                                let rgb = accent_rgb.get();
                                view! {
                                    <span
                                        class="text-[8px] font-mono uppercase tracking-[0.18em] leading-none"
                                        style=format!(
                                            "color: rgb({rgb}); \
                                             text-shadow: 0 0 8px rgba({rgb}, 0.6)"
                                        )
                                    >
                                        "● Now Playing"
                                    </span>
                                }.into_any()
                            } else {
                                let cat = category_for_meta.clone();
                                view! {
                                    <div class="flex items-center gap-1.5">
                                        <div
                                            class="w-1 h-1 rounded-full shrink-0"
                                            style=move || {
                                                let rgb = accent_rgb.get();
                                                format!(
                                                    "background: rgb({rgb}); \
                                                     box-shadow: 0 0 5px rgba({rgb}, 0.75)"
                                                )
                                            }
                                        />
                                        <span
                                            class="text-[8px] font-mono uppercase tracking-[0.16em] leading-none capitalize"
                                            style=move || format!(
                                                "color: rgba({}, 0.88); \
                                                 text-shadow: 0 1px 3px rgba(0, 0, 0, 0.9)",
                                                meta_tint.get()
                                            )
                                        >
                                            {cat}
                                        </span>
                                    </div>
                                }.into_any()
                            }
                        }}

                        // Title — palette-tinted, drop-shadowed for legibility.
                        <div
                            class="text-[12.5px] font-semibold truncate leading-tight \
                                   drop-shadow-[0_2px_6px_rgba(0,0,0,0.9)]"
                            style:color=move || format!("rgb({})", title_tint.get())
                        >
                            {name}
                        </div>

                        // Optional audio-reactive chip.
                        {audio_reactive.then(|| view! {
                            <span
                                class="inline-flex items-center gap-1 w-fit text-[8.5px] font-mono \
                                       px-1.5 py-[1px] rounded-full"
                                style="background: rgba(255, 106, 193, 0.16); \
                                       color: rgb(255, 180, 218); \
                                       border: 1px solid rgba(255, 106, 193, 0.28); \
                                       backdrop-filter: blur(2px)"
                                title="Audio-reactive"
                            >
                                <Icon icon=LuAudioLines width="8px" height="8px" />
                                "audio"
                            </span>
                        })}
                    </div>
                </button>

                // ── Floating remove button, revealed on hover.
                <button
                    class="absolute top-2 right-2 z-10 p-1 rounded-full \
                           opacity-0 group-hover:opacity-100 \
                           transition-all duration-200 hover:scale-110 active:scale-95"
                    style="background: rgba(0, 0, 0, 0.55); \
                           backdrop-filter: blur(4px); \
                           border: 1px solid rgba(255, 255, 255, 0.10)"
                    title="Remove from favorites"
                    aria-label="Remove from favorites"
                    on:click={
                        let fav_id = fav_id.clone();
                        move |ev: ev::MouseEvent| {
                            ev.stop_propagation();
                            fx.toggle_favorite(fav_id.clone());
                        }
                    }
                >
                    <Icon icon=LuX width="10px" height="10px" style="color: rgba(255, 255, 255, 0.9)" />
                </button>
            </div>
        </div>
    }
}
