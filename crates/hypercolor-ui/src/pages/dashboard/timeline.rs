//! Dashboard time-based panels — frame timeline, pacing, latest frame, backpressure.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::perf_charts::{PhaseFrame, PhaseWaterfall, Sparkline};
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::icons::*;
use crate::ws::{BackpressureNotice, PerformanceMetrics};

// ── Frame timeline (rolling phase waterfall) ────────────────────────

#[component]
pub(super) fn FrameTimelinePanel(
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
    #[prop(into)] phase_history: Signal<Vec<PhaseFrame>>,
) -> impl IntoView {
    let budget = Memo::new(move |_| {
        metrics
            .get()
            .map_or(33.33, |m| m.timeline.budget_ms.max(0.1))
    });
    let token_text = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| {
                format!(
                    "frame #{} · {} · {} layer{} · {} group{}",
                    m.timeline.frame_token,
                    m.timeline.compositor_backend.replace('_', " "),
                    m.timeline.logical_layer_count,
                    if m.timeline.logical_layer_count == 1 {
                        ""
                    } else {
                        "s"
                    },
                    m.timeline.render_group_count,
                    if m.timeline.render_group_count == 1 {
                        ""
                    } else {
                        "s"
                    },
                )
            })
            .unwrap_or_else(|| "waiting for frame".into())
    });
    let scene_badge = Memo::new(move |_| {
        metrics.get().map(|m| {
            let active = m.timeline.scene_active;
            let xfade = m.timeline.scene_transition_active;
            (active, xfade)
        })
    });

    view! {
        <div
            class="rounded-lg bg-surface-overlay/40 border border-transparent"
            style="border-top: 2px solid rgba(241, 250, 140, 0.25)"
        >
            <div class="px-4 py-2.5 flex items-center justify-between gap-2">
                <div class="flex items-center gap-2">
                    <Icon icon=LuTimer width="14px" height="14px" style="color: var(--color-electric-yellow)" />
                    <h2 class="text-[13px] font-medium text-fg-secondary">"Frame Timeline"</h2>
                    <span class="text-[9px] font-mono uppercase tracking-[0.1em] text-fg-tertiary/50">
                        "last 30s"
                    </span>
                </div>
                <div class="flex items-center gap-2">
                    {move || scene_badge.get().map(|(active, xfade)| view! {
                        <div class="flex items-center gap-1.5">
                            {active.then(|| view! {
                                <span class="text-[9px] font-mono uppercase tracking-[0.1em] px-1.5 py-0.5 rounded bg-electric-purple/10 text-electric-purple border border-electric-purple/20">
                                    "scene"
                                </span>
                            })}
                            {xfade.then(|| view! {
                                <span class="text-[9px] font-mono uppercase tracking-[0.1em] px-1.5 py-0.5 rounded bg-coral/10 text-coral border border-coral/20">
                                    "xfade"
                                </span>
                            })}
                        </div>
                    })}
                    <div class="text-[10px] font-mono text-fg-tertiary">
                        {move || token_text.get()}
                    </div>
                </div>
            </div>
            <div class="px-4 pb-4">
                <PhaseWaterfall frames=phase_history budget_ms=budget />
            </div>
        </div>
    }
}

// ── Pacing panel (sparklines) ────────────────────────────────────────

#[component]
pub(super) fn PacingPanel(
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
    #[prop(into)] jitter_series: Signal<Vec<f64>>,
    #[prop(into)] wake_series: Signal<Vec<f64>>,
    #[prop(into)] frame_age_series: Signal<Vec<f64>>,
    #[prop(into)] frame_p95_series: Signal<Vec<f64>>,
) -> impl IntoView {
    let jitter_label = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| {
                format!(
                    "p95 {:.2} ms · max {:.2} ms",
                    m.pacing.jitter_p95_ms, m.pacing.jitter_max_ms
                )
            })
            .unwrap_or_else(|| "—".into())
    });
    let wake_label = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| {
                format!(
                    "p95 {:.2} ms · max {:.2} ms",
                    m.pacing.wake_delay_p95_ms, m.pacing.wake_delay_max_ms
                )
            })
            .unwrap_or_else(|| "—".into())
    });
    let age_label = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("{:.2} ms", m.pacing.frame_age_ms))
            .unwrap_or_else(|| "—".into())
    });
    let frame_p95_label = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("{:.2} ms", m.frame_time.p95_ms))
            .unwrap_or_else(|| "—".into())
    });

    view! {
        <div class="pt-1">
            <div class="flex items-center gap-2 mb-3">
                <Icon icon=LuWifi width="14px" height="14px" style="color: var(--color-electric-purple)" />
                <h2 class="text-[13px] font-medium text-fg-secondary">"Frame Pacing"</h2>
            </div>
            <div class="space-y-4">
                <PacingRow
                    label="Jitter"
                    detail=Signal::derive(move || jitter_label.get())
                    values=jitter_series
                    color="var(--color-electric-purple)"
                />
                <PacingRow
                    label="Wake Delay"
                    detail=Signal::derive(move || wake_label.get())
                    values=wake_series
                    color="var(--color-electric-yellow)"
                />
                <PacingRow
                    label="Frame Age"
                    detail=Signal::derive(move || age_label.get())
                    values=frame_age_series
                    color="var(--color-neon-cyan)"
                />
                <PacingRow
                    label="Frame Time p95"
                    detail=Signal::derive(move || frame_p95_label.get())
                    values=frame_p95_series
                    color="var(--color-coral)"
                />
            </div>
        </div>
    }
}

#[component]
fn PacingRow(
    label: &'static str,
    #[prop(into)] detail: Signal<String>,
    #[prop(into)] values: Signal<Vec<f64>>,
    color: &'static str,
) -> impl IntoView {
    view! {
        <div class="space-y-1.5">
            <div class="flex items-baseline justify-between gap-2">
                <span class=label_class(LabelSize::Small, LabelTone::Default)>{label}</span>
                <span class="text-[11px] font-mono tabular-nums" style=format!("color: {color}")>
                    {move || detail.get()}
                </span>
            </div>
            <div class="h-10">
                <Sparkline values=values stroke=color />
            </div>
        </div>
    }
}

// ── Latest frame detail ──────────────────────────────────────────────

#[component]
pub(super) fn LatestFramePanel(
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
) -> impl IntoView {
    let line = Memo::new(move |_| {
        metrics
            .get()
            .and_then(|m| {
                if m.timeline.frame_token == 0 {
                    return None;
                }
                Some(format!(
                    "#{} · {} · wake {:.2} · snap {:.2} · input {:.2} · prod {:.2} · comp {:.2} · samp {:.2} · out {:.2} · pub {:.2} · frame {:.2}",
                    m.timeline.frame_token,
                    m.timeline.compositor_backend.replace('_', " "),
                    m.timeline.wake_late_ms,
                    m.timeline.scene_snapshot_done_ms,
                    m.timeline.input_done_ms,
                    m.timeline.producer_done_ms,
                    m.timeline.composition_done_ms,
                    m.timeline.sampling_done_ms,
                    m.timeline.output_done_ms,
                    m.timeline.publish_done_ms,
                    m.timeline.frame_done_ms,
                ))
            })
            .unwrap_or_else(|| "waiting for frame metadata".into())
    });

    view! {
        <div class="rounded-md bg-surface-overlay/25 px-4 py-3">
            <div class="flex items-center justify-between gap-3 mb-1">
                <div class="flex items-center gap-2">
                    <Icon icon=LuCode width="13px" height="13px" style="color: var(--color-fg-tertiary)" />
                    <span class=label_class(LabelSize::Small, LabelTone::Default)>"Latest Frame"</span>
                </div>
            </div>
            <div class="text-[11px] font-mono text-fg-secondary/90 break-all">
                {move || line.get()}
            </div>
        </div>
    }
}

#[component]
pub(super) fn BackpressureBanner(notice: BackpressureNotice) -> impl IntoView {
    let text = format!(
        "{} dropped on {}. {} → {} fps",
        notice.dropped_frames,
        notice.channel,
        notice.recommendation.replace('_', " "),
        notice.suggested_fps,
    );

    view! {
        <div class="rounded-xl border border-electric-yellow/25 bg-electric-yellow/[0.06] px-4 py-3 text-[12px] text-electric-yellow flex items-center gap-3">
            <Icon icon=LuTriangleAlert width="16px" height="16px" style="color: var(--color-electric-yellow); flex-shrink: 0" />
            <div>
                <span class="font-mono uppercase tracking-[0.14em] mr-2">"Backpressure"</span>
                {text}
            </div>
        </div>
    }
}
