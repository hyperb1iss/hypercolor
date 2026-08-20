//! Dashboard gauge panels — hero gauges, memory/devices, reuse rates, stat tiles.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::perf_charts::{HitRateBar, ProgressRing, Sparkline};
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::icons::*;
use crate::preview_telemetry::PreviewPresenterTelemetry;
use crate::ws::PerformanceMetrics;

use super::fps_display::stabilize_fps_for_display;

const EMA_ALPHA: f64 = 0.3;

fn use_ema(
    source: impl Fn() -> Option<f64> + Copy + Send + Sync + 'static,
    alpha: f64,
) -> Signal<f64> {
    let state = RwSignal::new(None::<f64>);
    Effect::new(move |_| {
        if let Some(raw) = source() {
            state.set(Some(match state.get_untracked() {
                None => raw,
                Some(prev) => prev + alpha * (raw - prev),
            }));
        }
    });
    Signal::derive(move || state.get().unwrap_or(0.0))
}

// ── Hero gauges: Engine FPS / Frame Time / Preview FPS ───────────────

#[component]
pub(super) fn HeroGauges(
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
    #[prop(into)] preview_fps: Signal<f32>,
    #[prop(into)] preview_target_fps: Signal<u32>,
    #[prop(into)] preview_present: Signal<PreviewPresenterTelemetry>,
    #[prop(into)] engine_fps_series: Signal<Vec<f64>>,
    #[prop(into)] frame_time_series: Signal<Vec<f64>>,
    #[prop(into)] preview_fps_series: Signal<Vec<f64>>,
) -> impl IntoView {
    // Engine FPS gauge values — EMA-smoothed for stable display
    let engine_raw = Memo::new(move |_| {
        metrics.with(|m| {
            m.as_ref()
                .map(|m| stabilize_fps_for_display(m.fps.delivered, m.fps.target))
        })
    });
    let engine_value = use_ema(move || engine_raw.get(), EMA_ALPHA);
    let engine_max = Memo::new(move |_| {
        metrics.with(|m| {
            m.as_ref()
                .map_or(60.0, |m| f64::from(m.fps.target).max(1.0))
        })
    });
    let engine_primary = Memo::new(move |_| {
        if metrics.with(Option::is_some) {
            format!("{:.1}", engine_value.get())
        } else {
            "—".into()
        }
    });
    let engine_secondary = Memo::new(move |_| {
        metrics.with(|m| {
            m.as_ref()
                .map(|m| format!("/ {} fps", m.fps.target))
                .unwrap_or_else(|| "waiting".into())
        })
    });

    // Frame time gauge — inverted: lower is better. EMA-smoothed.
    let frame_raw = Memo::new(move |_| metrics.with(|m| m.as_ref().map(|m| m.frame_time.avg_ms)));
    let frame_value = use_ema(move || frame_raw.get(), EMA_ALPHA);
    let frame_budget = Memo::new(move |_| {
        metrics.with(|m| {
            m.as_ref().map_or(33.33, |m| {
                if m.fps.target > 0 {
                    1000.0 / f64::from(m.fps.target)
                } else {
                    33.33
                }
            })
        })
    });
    let frame_primary = Memo::new(move |_| {
        if metrics.with(Option::is_some) {
            format!("{:.2}", frame_value.get())
        } else {
            "—".into()
        }
    });
    let frame_secondary = Memo::new(move |_| {
        metrics.with(|m| {
            m.as_ref()
                .map(|m| {
                    format!(
                        "/ {:.1} ms",
                        if m.fps.target > 0 {
                            1000.0 / f64::from(m.fps.target)
                        } else {
                            33.33
                        }
                    )
                })
                .unwrap_or_else(|| "ms".into())
        })
    });

    // Preview gauge — EMA-smoothed
    let preview_raw = Memo::new(move |_| {
        let stream_fps = f64::from(preview_fps.get());
        let present_fps = f64::from(preview_present.get().present_fps);
        let fps = if stream_fps > 0.0 {
            stream_fps
        } else {
            present_fps
        };
        let fps = stabilize_fps_for_display(fps, preview_target_fps.get());
        if fps > 0.0 { Some(fps) } else { None }
    });
    let preview_value = use_ema(move || preview_raw.get(), EMA_ALPHA);
    let preview_primary = Memo::new(move |_| format!("{:.1}", preview_value.get()));
    let preview_secondary = Memo::new(move |_| {
        let target = preview_target_fps.get();
        let present = preview_present.get();
        let mode = present.runtime_mode.unwrap_or("pending");
        let arrival = present.arrival_to_present_ms;
        if arrival > 0.0 {
            format!("/ {target} · {mode} · {arrival:.1}ms")
        } else {
            format!("/ {target} · {mode}")
        }
    });

    // Health-colored dropped badge
    let dropped_text = Memo::new(move |_| {
        metrics.with(|m| {
            m.as_ref()
                .map(|m| {
                    format!(
                        "{} budget miss{}",
                        m.fps.dropped,
                        if m.fps.dropped == 1 { "" } else { "es" }
                    )
                })
                .unwrap_or_else(|| "metrics warming up".into())
        })
    });

    // Health gates for the warning tint: engine within 10% of target,
    // frame time inside budget. Preview and clients stay neutral.
    let engine_healthy = Memo::new(move |_| engine_value.get() >= engine_max.get() * 0.9);
    let frame_healthy = Memo::new(move |_| frame_value.get() <= frame_budget.get());
    let ws_clients =
        Memo::new(move |_| metrics.with(|m| m.as_ref().map_or(0, |m| m.websocket.client_count)));

    view! {
        <div
            class="rounded-lg bg-surface-overlay/40 border border-transparent"
            style="border-top: 2px solid rgba(128, 255, 234, 0.30)"
        >
            <div class="px-4 py-2.5 flex items-center justify-between">
                <div class="flex items-center gap-2">
                    <Icon icon=LuActivity width="14px" height="14px" style="color: var(--color-neon-cyan)" />
                    <h2 class="text-[13px] font-medium text-fg-secondary">"Performance"</h2>
                </div>
                <div class="text-[10px] font-mono text-fg-tertiary/70">
                    {move || dropped_text.get()}
                </div>
            </div>
            <div class="px-4 pb-4 grid grid-cols-4 gap-2 max-md:grid-cols-2 max-md:px-3 max-md:pb-3">
                <StatTile
                    label="Engine"
                    value=engine_primary
                    detail=engine_secondary
                    series=engine_fps_series
                    color="var(--color-neon-cyan)"
                    healthy=engine_healthy
                />
                <StatTile
                    label="Frame"
                    value=frame_primary
                    detail=frame_secondary
                    series=frame_time_series
                    color="var(--color-electric-purple)"
                    healthy=frame_healthy
                />
                <StatTile
                    label="Preview"
                    value=preview_primary
                    detail=preview_secondary
                    series=preview_fps_series
                    color="var(--color-coral)"
                />
                <StatTile
                    label="Clients"
                    value=Memo::new(move |_| ws_clients.get().to_string())
                    detail=Signal::derive(|| "ws connected".to_owned())
                    color="var(--color-electric-yellow)"
                />
            </div>
        </div>
    }
}

/// One KPI in the performance strip: a hero number with its context line
/// and a thin sparkline. The colored dot carries series identity so the
/// number itself stays in text tokens; it only takes the warning tint
/// when `healthy` reports false, and the detail line's target/budget
/// text says why, so state is never color-alone.
#[component]
fn StatTile(
    label: &'static str,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] detail: Signal<String>,
    #[prop(optional, into)] series: Option<Signal<Vec<f64>>>,
    #[prop(default = "var(--color-neon-cyan)")] color: &'static str,
    #[prop(optional, into)] healthy: Option<Signal<bool>>,
) -> impl IntoView {
    let value_style = move || match healthy {
        Some(h) if !h.get() => "color: var(--color-electric-yellow)",
        _ => "",
    };

    view! {
        <div class="rounded-md bg-surface-overlay/20 px-3 pt-2.5 pb-2 flex flex-col gap-1 min-w-0">
            <div class="flex items-center justify-between gap-2">
                <span class="text-[9px] font-mono uppercase tracking-[0.16em] text-fg-tertiary">
                    {label}
                </span>
                <span
                    class="w-1.5 h-1.5 rounded-full shrink-0"
                    style=format!("background: {color}; box-shadow: 0 0 6px {color}")
                />
            </div>
            <div
                class="text-xl font-semibold tabular-nums leading-none text-fg-primary"
                style=value_style
            >
                {move || value.get()}
            </div>
            <div class="text-[10px] font-mono text-fg-tertiary/70 truncate">
                {move || detail.get()}
            </div>
            {series.map(|s| view! {
                <div class="h-7 mt-0.5">
                    <Sparkline values=s stroke=color />
                </div>
            })}
        </div>
    }
}

// ── Reuse rates ──────────────────────────────────────────────────────

#[component]
pub(super) fn ReuseRatesPanel(
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
) -> impl IntoView {
    // Max reuse count over a 120-frame window is 120.
    let window = Signal::derive(|| 120_u32);

    let reused_inputs =
        Memo::new(move |_| metrics.with(|m| m.as_ref().map_or(0, |m| m.pacing.reused_inputs)));
    let reused_canvas =
        Memo::new(move |_| metrics.with(|m| m.as_ref().map_or(0, |m| m.pacing.reused_canvas)));
    let retained_effect =
        Memo::new(move |_| metrics.with(|m| m.as_ref().map_or(0, |m| m.pacing.retained_effect)));
    let retained_screen =
        Memo::new(move |_| metrics.with(|m| m.as_ref().map_or(0, |m| m.pacing.retained_screen)));
    let composition_bypassed = Memo::new(move |_| {
        metrics.with(|m| m.as_ref().map_or(0, |m| m.pacing.composition_bypassed))
    });

    view! {
        <div
            class="rounded-lg bg-surface-overlay/40 border border-transparent"
            style="border-top: 2px solid rgba(80, 250, 123, 0.25)"
        >
            <div class="px-4 py-2.5 flex items-center justify-between">
                <div class="flex items-center gap-2">
                    <Icon icon=LuZap width="14px" height="14px" style="color: var(--color-success-green)" />
                    <h2 class="text-[13px] font-medium text-fg-secondary">"Reuse Efficiency"</h2>
                </div>
                <div class="text-[10px] font-mono text-fg-tertiary/70">"120-frame window"</div>
            </div>
            <div class="p-4 space-y-3">
                <HitRateBar
                    label=Signal::derive(|| "Input reuse".to_string())
                    value=reused_inputs
                    total=window
                    color="var(--color-success-green)"
                />
                <HitRateBar
                    label=Signal::derive(|| "Canvas reuse".to_string())
                    value=reused_canvas
                    total=window
                    color="var(--color-neon-cyan)"
                />
                <HitRateBar
                    label=Signal::derive(|| "Effect retained".to_string())
                    value=retained_effect
                    total=window
                    color="var(--color-electric-purple)"
                />
                <HitRateBar
                    label=Signal::derive(|| "Screen retained".to_string())
                    value=retained_screen
                    total=window
                    color="var(--color-coral)"
                />
                <HitRateBar
                    label=Signal::derive(|| "Composition bypassed".to_string())
                    value=composition_bypassed
                    total=window
                    color="var(--color-electric-yellow)"
                />
            </div>
        </div>
    }
}

// ── Memory & Devices ─────────────────────────────────────────────────

#[component]
pub(super) fn MemoryAndDevicesPanel(
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
) -> impl IntoView {
    // Soft caps for progress rings. The daemon has no hard ceiling, so we use
    // a generous reference point so the ring is a visual gauge rather than a
    // "percent of limit" reading.
    let daemon_rss =
        Memo::new(move |_| metrics.with(|m| m.as_ref().map_or(0.0, |m| m.memory.daemon_rss_mb)));
    let canvas_kb =
        Memo::new(move |_| metrics.with(|m| m.as_ref().map_or(0, |m| m.memory.canvas_buffer_kb)));

    let daemon_detail = Memo::new(move |_| format!("{:.1} MB", daemon_rss.get()));
    let canvas_detail = Memo::new(move |_| format!("{} KB", canvas_kb.get()));

    // Soft reference ceilings — the daemon process (which includes the
    // in-process Servo runtime) has no hard cap, so the ring reads as a
    // rough gauge, not a percent-of-limit.
    let daemon_max = Signal::derive(|| 1024.0_f64);
    let canvas_max = Signal::derive(|| 1024.0_f64);

    let device_count =
        Memo::new(move |_| metrics.with(|m| m.as_ref().map_or(0, |m| m.devices.connected)));
    let total_leds =
        Memo::new(move |_| metrics.with(|m| m.as_ref().map_or(0, |m| m.devices.total_leds)));
    let output_errors =
        Memo::new(move |_| metrics.with(|m| m.as_ref().map_or(0, |m| m.devices.output_errors)));

    let errors_color = Memo::new(move |_| {
        let e = output_errors.get();
        if e == 0 {
            "var(--color-success-green)"
        } else if e < 10 {
            "var(--color-electric-yellow)"
        } else {
            "var(--color-error-red)"
        }
    });

    view! {
        <div
            class="rounded-lg bg-surface-overlay/40 border border-transparent"
            style="border-top: 2px solid rgba(255, 106, 193, 0.25)"
        >
            <div class="px-4 py-2.5 flex items-center gap-2">
                <Icon icon=LuCpu width="14px" height="14px" style="color: var(--color-coral)" />
                <h2 class="text-[13px] font-medium text-fg-secondary">"Memory & Devices"</h2>
            </div>
            <div class="p-4 space-y-4">
                <div class="space-y-3">
                    <ProgressRing
                        value=daemon_rss
                        max=daemon_max
                        label=Signal::derive(|| "App memory".to_string())
                        detail=daemon_detail
                        color="var(--color-electric-purple)"
                    />
                    <ProgressRing
                        value=Memo::new(move |_| f64::from(canvas_kb.get()))
                        max=canvas_max
                        label=Signal::derive(|| "Canvas buffer".to_string())
                        detail=canvas_detail
                        color="var(--color-coral)"
                    />
                </div>
                <div class="border-t border-edge-subtle pt-4 grid grid-cols-3 gap-3">
                    <StatMini
                        label="Devices"
                        value=Memo::new(move |_| device_count.get().to_string())
                        color="var(--color-coral)"
                    />
                    <StatMini
                        label="LEDs"
                        value=Memo::new(move |_| total_leds.get().to_string())
                        color="var(--color-neon-cyan)"
                    />
                    <StatMini
                        label="Errors"
                        value=Memo::new(move |_| output_errors.get().to_string())
                        color_signal=Signal::from(errors_color)
                    />
                </div>
            </div>
        </div>
    }
}

#[component]
fn StatMini(
    label: &'static str,
    #[prop(into)] value: Signal<String>,
    #[prop(default = "var(--color-fg-primary)")] color: &'static str,
    #[prop(optional)] color_signal: Option<Signal<&'static str>>,
) -> impl IntoView {
    view! {
        <div class="rounded-md bg-surface-overlay/20 px-3 py-2 text-center">
            <div class=label_class(LabelSize::Micro, LabelTone::Default)>{label}</div>
            <div
                class="text-[16px] font-semibold tabular-nums mt-0.5"
                style=move || format!("color: {}", color_signal.map_or(color, |s| s.get()))
            >
                {move || value.get()}
            </div>
        </div>
    }
}
