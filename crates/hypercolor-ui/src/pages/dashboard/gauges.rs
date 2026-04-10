//! Dashboard gauge panels — hero gauges, memory/devices, reuse rates, stat tiles.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::perf_charts::{HitRateBar, ProgressRing, RadialGauge, Sparkline};
use crate::icons::*;
use crate::preview_telemetry::PreviewPresenterTelemetry;
use crate::ws::PerformanceMetrics;

const EMA_ALPHA: f64 = 0.3;

fn use_ema(source: impl Fn() -> Option<f64> + Copy + Send + Sync + 'static, alpha: f64) -> Signal<f64> {
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
    let engine_raw = Memo::new(move |_| metrics.get().map(|m| m.fps.actual));
    let engine_value = use_ema(move || engine_raw.get(), EMA_ALPHA);
    let engine_max = Memo::new(move |_| metrics.get().map_or(60.0, |m| f64::from(m.fps.target).max(1.0)));
    let engine_primary = Memo::new(move |_| {
        if metrics.get().is_some() {
            format!("{:.1}", engine_value.get())
        } else {
            "—".into()
        }
    });
    let engine_secondary = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("/ {} fps", m.fps.target))
            .unwrap_or_else(|| "waiting".into())
    });

    // Frame time gauge — inverted: lower is better. EMA-smoothed.
    let frame_raw = Memo::new(move |_| metrics.get().map(|m| m.frame_time.avg_ms));
    let frame_value = use_ema(move || frame_raw.get(), EMA_ALPHA);
    let frame_budget = Memo::new(move |_| {
        metrics.get().map_or(33.33, |m| {
            if m.fps.target > 0 {
                1000.0 / f64::from(m.fps.target)
            } else {
                33.33
            }
        })
    });
    let frame_primary = Memo::new(move |_| {
        if metrics.get().is_some() {
            format!("{:.2}", frame_value.get())
        } else {
            "—".into()
        }
    });
    let frame_secondary = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("/ {:.1} ms", if m.fps.target > 0 { 1000.0 / f64::from(m.fps.target) } else { 33.33 }))
            .unwrap_or_else(|| "ms".into())
    });

    // Preview gauge — EMA-smoothed
    let preview_raw = Memo::new(move |_| {
        let present = preview_present.get().present_fps;
        let fps = if present > 0.0 {
            f64::from(present)
        } else {
            f64::from(preview_fps.get())
        };
        if fps > 0.0 { Some(fps) } else { None }
    });
    let preview_value = use_ema(move || preview_raw.get(), EMA_ALPHA);
    let preview_max = Memo::new(move |_| f64::from(preview_target_fps.get()).max(1.0));
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
        metrics
            .get()
            .map(|m| format!("{} budget miss{}", m.fps.dropped, if m.fps.dropped == 1 { "" } else { "es" }))
            .unwrap_or_else(|| "metrics warming up".into())
    });

    view! {
        <div
            class="rounded-lg bg-surface-overlay/40 border border-transparent"
            style="border-top: 2px solid rgba(128, 255, 234, 0.30)"
        >
            <div class="px-4 py-2.5 flex items-center justify-between">
                <div class="flex items-center gap-2">
                    <Icon icon=LuActivity width="14px" height="14px" style="color: var(--color-neon-cyan)" />
                    <h2 class="text-[13px] font-medium text-fg-secondary">"Render Engine"</h2>
                </div>
                <div class="text-[10px] font-mono text-fg-tertiary/70">
                    {move || dropped_text.get()}
                </div>
            </div>
            <div class="px-4 pb-4 grid grid-cols-1 md:grid-cols-3 gap-3">
                <GaugeWithSparkline
                    caption="Engine"
                    gauge_value=Signal::derive(move || engine_value.get())
                    gauge_max=Signal::derive(move || engine_max.get())
                    primary=Signal::derive(move || engine_primary.get())
                    secondary=Signal::derive(move || engine_secondary.get())
                    gauge_color="var(--color-neon-cyan)"
                    sparkline_values=engine_fps_series
                    sparkline_color="var(--color-neon-cyan)"
                />
                <GaugeWithSparkline
                    caption="Frame Time"
                    gauge_value=Signal::derive(move || {
                        // Invert: budget - actual, so ring fills more when we have headroom.
                        let b = frame_budget.get();
                        (b - frame_value.get()).max(0.0)
                    })
                    gauge_max=Signal::derive(move || frame_budget.get())
                    primary=Signal::derive(move || frame_primary.get())
                    secondary=Signal::derive(move || frame_secondary.get())
                    gauge_color="var(--color-electric-purple)"
                    sparkline_values=frame_time_series
                    sparkline_color="var(--color-electric-purple)"
                />
                <GaugeWithSparkline
                    caption="Preview"
                    gauge_value=Signal::derive(move || preview_value.get())
                    gauge_max=Signal::derive(move || preview_max.get())
                    primary=Signal::derive(move || preview_primary.get())
                    secondary=Signal::derive(move || preview_secondary.get())
                    gauge_color="var(--color-coral)"
                    sparkline_values=preview_fps_series
                    sparkline_color="var(--color-coral)"
                />
            </div>
        </div>
    }
}

#[component]
fn GaugeWithSparkline(
    caption: &'static str,
    #[prop(into)] gauge_value: Signal<f64>,
    #[prop(into)] gauge_max: Signal<f64>,
    #[prop(into)] primary: Signal<String>,
    #[prop(into)] secondary: Signal<String>,
    gauge_color: &'static str,
    #[prop(into)] sparkline_values: Signal<Vec<f64>>,
    sparkline_color: &'static str,
) -> impl IntoView {
    view! {
        <div class="rounded-md bg-surface-overlay/20 px-3 py-3 flex flex-col items-center gap-2">
            <RadialGauge
                caption=caption
                value=gauge_value
                max=gauge_max
                primary=primary
                secondary=secondary
                color=gauge_color
            />
            <div class="w-full h-12">
                <Sparkline
                    values=sparkline_values
                    stroke=sparkline_color
                />
            </div>
        </div>
    }
}

// ── Reuse rates ──────────────────────────────────────────────────────

#[component]
pub(super) fn ReuseRatesPanel(#[prop(into)] metrics: Signal<Option<PerformanceMetrics>>) -> impl IntoView {
    // Max reuse count over a 120-frame window is 120.
    let window = Signal::derive(|| 120_u32);

    let reused_inputs = Memo::new(move |_| metrics.get().map_or(0, |m| m.pacing.reused_inputs));
    let reused_canvas = Memo::new(move |_| metrics.get().map_or(0, |m| m.pacing.reused_canvas));
    let retained_effect = Memo::new(move |_| metrics.get().map_or(0, |m| m.pacing.retained_effect));
    let retained_screen = Memo::new(move |_| metrics.get().map_or(0, |m| m.pacing.retained_screen));
    let composition_bypassed = Memo::new(move |_| metrics.get().map_or(0, |m| m.pacing.composition_bypassed));

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
                    value=Signal::derive(move || reused_inputs.get())
                    total=window
                    color="var(--color-success-green)"
                />
                <HitRateBar
                    label=Signal::derive(|| "Canvas reuse".to_string())
                    value=Signal::derive(move || reused_canvas.get())
                    total=window
                    color="var(--color-neon-cyan)"
                />
                <HitRateBar
                    label=Signal::derive(|| "Effect retained".to_string())
                    value=Signal::derive(move || retained_effect.get())
                    total=window
                    color="var(--color-electric-purple)"
                />
                <HitRateBar
                    label=Signal::derive(|| "Screen retained".to_string())
                    value=Signal::derive(move || retained_screen.get())
                    total=window
                    color="var(--color-coral)"
                />
                <HitRateBar
                    label=Signal::derive(|| "Composition bypassed".to_string())
                    value=Signal::derive(move || composition_bypassed.get())
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
    let daemon_rss = Memo::new(move |_| metrics.get().map_or(0.0, |m| m.memory.daemon_rss_mb));
    let servo_rss = Memo::new(move |_| metrics.get().map_or(0.0, |m| m.memory.servo_rss_mb));
    let canvas_kb = Memo::new(move |_| metrics.get().map_or(0, |m| m.memory.canvas_buffer_kb));

    let daemon_detail = Memo::new(move |_| format!("{:.1} MB", daemon_rss.get()));
    let servo_detail = Memo::new(move |_| format!("{:.1} MB", servo_rss.get()));
    let canvas_detail = Memo::new(move |_| format!("{} KB", canvas_kb.get()));

    let daemon_max = Signal::derive(|| 512.0_f64);
    let servo_max = Signal::derive(|| 1024.0_f64);
    let canvas_max = Signal::derive(|| 1024.0_f64);

    let device_count = Memo::new(move |_| metrics.get().map_or(0, |m| m.devices.connected));
    let total_leds = Memo::new(move |_| metrics.get().map_or(0, |m| m.devices.total_leds));
    let output_errors = Memo::new(move |_| metrics.get().map_or(0, |m| m.devices.output_errors));

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
                        value=Signal::derive(move || daemon_rss.get())
                        max=daemon_max
                        label=Signal::derive(|| "Daemon RSS".to_string())
                        detail=Signal::derive(move || daemon_detail.get())
                        color="var(--color-electric-purple)"
                    />
                    <ProgressRing
                        value=Signal::derive(move || servo_rss.get())
                        max=servo_max
                        label=Signal::derive(|| "Servo RSS".to_string())
                        detail=Signal::derive(move || servo_detail.get())
                        color="var(--color-neon-cyan)"
                    />
                    <ProgressRing
                        value=Signal::derive(move || f64::from(canvas_kb.get()))
                        max=canvas_max
                        label=Signal::derive(|| "Canvas buffer".to_string())
                        detail=Signal::derive(move || canvas_detail.get())
                        color="var(--color-coral)"
                    />
                </div>
                <div class="border-t border-edge-subtle pt-4 grid grid-cols-3 gap-3">
                    <StatMini
                        label="Devices"
                        value=Signal::derive(move || device_count.get().to_string())
                        color="var(--color-coral)"
                    />
                    <StatMini
                        label="LEDs"
                        value=Signal::derive(move || total_leds.get().to_string())
                        color="var(--color-neon-cyan)"
                    />
                    <StatMini
                        label="Errors"
                        value=Signal::derive(move || output_errors.get().to_string())
                        color_signal=Signal::derive(move || errors_color.get())
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
            <div class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary">{label}</div>
            <div
                class="text-[16px] font-semibold tabular-nums mt-0.5"
                style=move || format!("color: {}", color_signal.map_or(color, |s| s.get()))
            >
                {move || value.get()}
            </div>
        </div>
    }
}
