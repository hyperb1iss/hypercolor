//! Dashboard chart panels — pipeline, distribution, throughput, favorites, resize hint.

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::EffectSummary;
use crate::app::EffectsContext;
use crate::components::perf_charts::{DistributionBar, Sparkline, StackSegment, StackedBar};
use crate::icons::*;
use crate::style_utils::category_style;
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

// ── Resize hint ──────────────────────────────────────────────────────

/// Tiny affordance at the bottom of the sidebar explaining that the split is
/// draggable. Pure discoverability — users kept missing the 2px handle.
#[component]
pub(super) fn ResizeHint() -> impl IntoView {
    view! {
        <div class="shrink-0 flex items-center justify-center gap-1.5 py-1.5 text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary/50 hover:text-electric-purple/70 transition-colors">
            <Icon icon=LuChevronLeft width="10px" height="10px" />
            <span>"drag edge to resize"</span>
            <Icon icon=LuChevronRight width="10px" height="10px" />
        </div>
    }
}

// ── Favorites (compact) ──────────────────────────────────────────────

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
            class="rounded-lg bg-surface-overlay/40 border border-transparent flex flex-col flex-1 min-h-0"
            style="border-top: 2px solid rgba(255, 106, 193, 0.25)"
        >
            <div class="px-4 py-2.5 border-b border-edge-subtle flex items-center justify-between">
                <div class="flex items-center gap-2">
                    <Icon icon=LuHeart width="12px" height="12px" style="color: var(--color-coral)" />
                    <h2 class="text-[12px] font-medium text-fg-secondary">"Favorites"</h2>
                </div>
                <span class="text-[10px] font-mono text-coral/80 rounded-full border border-coral/15 bg-coral/[0.06] px-2 py-0.5">
                    {move || favorites_count.get().to_string()}
                </span>
            </div>
            <div class="flex-1 overflow-y-auto p-2 min-h-0 max-h-[320px]">
                {move || {
                    let effects = favorite_effects.get();
                    if effects.is_empty() {
                        view! {
                            <div class="flex flex-col items-center justify-center h-full py-8 text-center">
                                <div class="w-10 h-10 rounded-xl bg-coral/[0.06] border border-coral/10 flex items-center justify-center mb-2">
                                    <Icon icon=LuHeart width="16px" height="16px" style="color: var(--color-coral); opacity: 0.4" />
                                </div>
                                <p class="text-[11px] text-fg-tertiary">"No favorites yet"</p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="space-y-1">
                                {effects.into_iter().enumerate().map(|(i, effect)| {
                                    let delay = format!("animation-delay: {}ms", i * 30);
                                    view! { <FavoriteRow effect=effect delay=delay /> }
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn FavoriteRow(effect: EffectSummary, delay: String) -> impl IntoView {
    let fx = expect_context::<EffectsContext>();
    let apply_id = effect.id.clone();
    let fav_id = effect.id.clone();
    let active_check_id = effect.id.clone();
    let name = effect.name.clone();
    let category = effect.category.clone();
    let audio_reactive = effect.audio_reactive;
    let (badge_class, accent_rgb) = category_style(&category);

    let is_active = Signal::derive(move || {
        fx.active_effect_id.get().as_deref() == Some(active_check_id.as_str())
    });

    let accent_border = format!("border-left: 2px solid rgba({accent_rgb}, 0.5)");
    let active_accent_border = format!("border-left: 2px solid rgba({accent_rgb}, 0.9)");

    view! {
        <div class="animate-fade-in-up" style=delay>
            <button
                class=move || {
                    let base = "w-full flex items-center gap-2 px-2.5 py-2 rounded-lg text-left group \
                                transition-all duration-150 btn-press";
                    if is_active.get() {
                        format!("{base} bg-accent-subtle border border-accent-muted")
                    } else {
                        format!("{base} bg-surface-overlay/20 border border-transparent \
                                 hover:bg-surface-hover/40 hover:border-edge-subtle")
                    }
                }
                style=move || if is_active.get() {
                    active_accent_border.clone()
                } else {
                    accent_border.clone()
                }
                on:click={
                    let apply_id = apply_id.clone();
                    move |_| fx.apply_effect(apply_id.clone())
                }
            >
                {move || is_active.get().then(|| view! {
                    <div class="w-1.5 h-1.5 rounded-full bg-accent animate-pulse shrink-0" />
                })}

                <div class="flex-1 min-w-0">
                    <div class="text-[11px] text-fg-secondary truncate group-hover:text-fg-primary transition-colors">
                        {name.clone()}
                    </div>
                    <div class="flex items-center gap-1.5 mt-0.5">
                        <span class=format!(
                            "text-[9px] font-mono px-1.5 py-0.5 rounded capitalize {}",
                            badge_class
                        )>
                            {category.clone()}
                        </span>
                        {audio_reactive.then(|| view! {
                            <span class="text-[9px] font-mono text-coral/70">
                                <Icon icon=LuAudioLines width="9px" height="9px" />
                            </span>
                        })}
                    </div>
                </div>

                <button
                    class="shrink-0 p-1 rounded opacity-0 group-hover:opacity-100 \
                           hover:bg-surface-hover/60 transition-all duration-150"
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
                    <Icon icon=LuX width="10px" height="10px" style="color: var(--color-fg-tertiary)" />
                </button>
            </button>
        </div>
    }
}
