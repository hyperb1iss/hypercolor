//! SVG-based performance visualization primitives for the dashboard.
//!
//! Pure inline SVG — no external charting deps. Every component accepts a
//! Leptos `Signal` so charts re-render cheaply when telemetry updates.

use leptos::prelude::*;

// ── Sparkline ────────────────────────────────────────────────────────

/// Tiny inline time-series chart. Draws a smooth polyline + soft area fill.
/// Automatically scales to the min/max of the supplied samples.
#[component]
pub fn Sparkline(
    /// Raw samples — oldest first.
    #[prop(into)]
    values: Signal<Vec<f64>>,
    /// Stroke color (CSS color expression).
    #[prop(default = "var(--color-neon-cyan)")]
    stroke: &'static str,
    /// Whether to render a soft area fill under the line.
    #[prop(default = true)]
    fill: bool,
    /// Optional dashed horizontal reference line in data-space.
    #[prop(default = None)]
    baseline: Option<f64>,
    /// Optional extra Tailwind classes for the wrapping svg element.
    #[prop(default = "")]
    class: &'static str,
) -> impl IntoView {
    const W: f64 = 200.0;
    const H: f64 = 48.0;
    const PAD_Y: f64 = 3.0;

    let geometry = Memo::new(move |_| {
        let vs = values.get();
        if vs.len() < 2 {
            return None;
        }
        let n = vs.len();
        let (mut lo, mut hi) = vs
            .iter()
            .copied()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                (a.min(v), b.max(v))
            });
        if let Some(b) = baseline {
            lo = lo.min(b);
            hi = hi.max(b);
        }
        if !lo.is_finite() || !hi.is_finite() {
            return None;
        }
        let range = (hi - lo).max(1e-9);
        let step = W / (n - 1) as f64;
        let project_y = |v: f64| -> f64 {
            let t = (v - lo) / range;
            H - PAD_Y - t * (H - PAD_Y * 2.0)
        };

        let mut line = String::with_capacity(n * 12);
        for (i, v) in vs.iter().enumerate() {
            let x = i as f64 * step;
            let y = project_y(*v);
            if i == 0 {
                line.push_str(&format!("M{x:.1},{y:.1}"));
            } else {
                line.push_str(&format!(" L{x:.1},{y:.1}"));
            }
        }
        let mut area = line.clone();
        area.push_str(&format!(" L{:.1},{:.1}", (n - 1) as f64 * step, H));
        area.push_str(&format!(" L0,{H} Z"));

        let baseline_y = baseline.map(project_y);
        let last_point = vs.last().copied().map(|v| ((n - 1) as f64 * step, project_y(v)));

        Some((line, area, baseline_y, last_point, lo, hi))
    });

    view! {
        <svg
            class=format!("block w-full h-full {class}")
            viewBox=format!("0 0 {W} {H}")
            preserveAspectRatio="none"
            aria-hidden="true"
        >
            {move || geometry.get().map(|(line, area, baseline_y, last_point, _lo, _hi)| {
                view! {
                    {fill.then(|| view! {
                        <path d=area.clone() fill=stroke fill-opacity="0.14" />
                    })}
                    {baseline_y.map(|y| view! {
                        <line
                            x1="0"
                            y1=format!("{y:.1}")
                            x2=format!("{W}")
                            y2=format!("{y:.1}")
                            stroke="var(--color-edge-default)"
                            stroke-width="0.8"
                            stroke-dasharray="2 3"
                            opacity="0.55"
                        />
                    })}
                    <path
                        d=line
                        fill="none"
                        stroke=stroke
                        stroke-width="1.6"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                    />
                    {last_point.map(|(x, y)| view! {
                        <circle cx=format!("{x:.1}") cy=format!("{y:.1}") r="2.2" fill=stroke />
                        <circle cx=format!("{x:.1}") cy=format!("{y:.1}") r="4.5" fill=stroke fill-opacity="0.2" />
                    })}
                }
            })}
        </svg>
    }
}

// ── Radial gauge ─────────────────────────────────────────────────────

/// Ring-shaped gauge with a center-aligned value label.
/// `value / max` determines the fill sweep; `color` is CSS.
#[component]
pub fn RadialGauge(
    #[prop(into)] value: Signal<f64>,
    #[prop(into)] max: Signal<f64>,
    /// Primary label inside the ring (e.g. "42.3").
    #[prop(into)]
    primary: Signal<String>,
    /// Secondary label below primary (e.g. "fps" or "/ 60").
    #[prop(into)]
    secondary: Signal<String>,
    #[prop(default = "var(--color-neon-cyan)")] color: &'static str,
    /// Optional small caption above the gauge (upper-case track label).
    #[prop(default = "")]
    caption: &'static str,
) -> impl IntoView {
    const SIZE: f64 = 120.0;
    const STROKE: f64 = 9.0;
    const RADIUS: f64 = (SIZE - STROKE) / 2.0;
    let circumference = 2.0 * std::f64::consts::PI * RADIUS;

    let pct = Memo::new(move |_| {
        let m = max.get();
        if m <= 0.0 {
            0.0
        } else {
            (value.get() / m).clamp(0.0, 1.05)
        }
    });
    let dash_offset = Memo::new(move |_| {
        let p = pct.get().min(1.0);
        circumference * (1.0 - p)
    });

    view! {
        <div class="flex flex-col items-center gap-1">
            {(!caption.is_empty()).then(|| view! {
                <div class="text-[9px] font-mono uppercase tracking-[0.16em] text-fg-tertiary">
                    {caption}
                </div>
            })}
            <div class="relative" style=format!("width: {SIZE}px; height: {SIZE}px; overflow: visible")>
                <svg
                    width=SIZE
                    height=SIZE
                    viewBox=format!("0 0 {SIZE} {SIZE}")
                    class="block -rotate-90"
                    style="overflow: visible"
                    aria-hidden="true"
                >
                    // Track
                    <circle
                        cx=SIZE / 2.0
                        cy=SIZE / 2.0
                        r=RADIUS
                        fill="none"
                        stroke="var(--color-edge-subtle)"
                        stroke-width=STROKE
                        stroke-opacity="0.45"
                    />
                    // Soft glow behind the fill
                    <circle
                        cx=SIZE / 2.0
                        cy=SIZE / 2.0
                        r=RADIUS
                        fill="none"
                        stroke=color
                        stroke-width=STROKE + 4.0
                        stroke-opacity="0.08"
                        stroke-linecap="round"
                        stroke-dasharray=format!("{circumference:.2}")
                        stroke-dashoffset=move || format!("{:.2}", dash_offset.get())
                        style="transition: stroke-dashoffset 0.45s cubic-bezier(0.4, 0, 0.2, 1)"
                    />
                    // Active arc
                    <circle
                        cx=SIZE / 2.0
                        cy=SIZE / 2.0
                        r=RADIUS
                        fill="none"
                        stroke=color
                        stroke-width=STROKE
                        stroke-linecap="round"
                        stroke-dasharray=format!("{circumference:.2}")
                        stroke-dashoffset=move || format!("{:.2}", dash_offset.get())
                        style="transition: stroke-dashoffset 0.45s cubic-bezier(0.4, 0, 0.2, 1); filter: drop-shadow(0 0 6px currentColor)"
                    />
                </svg>
                <div class="absolute inset-0 flex flex-col items-center justify-center pointer-events-none">
                    <div
                        class="text-[22px] font-semibold tabular-nums leading-none"
                        style=format!("color: {color}")
                    >
                        {move || primary.get()}
                    </div>
                    <div class="mt-1 text-[10px] font-mono text-fg-tertiary uppercase tracking-[0.12em]">
                        {move || secondary.get()}
                    </div>
                </div>
            </div>
        </div>
    }
}

// ── Stacked horizontal bar ───────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct StackSegment {
    pub label: &'static str,
    pub value: f64,
    pub color: &'static str,
}

/// A horizontal stacked bar — segments sized proportionally to their value.
/// Includes a floating label row underneath showing values.
#[component]
pub fn StackedBar(
    #[prop(into)] segments: Signal<Vec<StackSegment>>,
    /// Optional explicit total; defaults to sum of segment values.
    #[prop(default = None)]
    total_override: Option<f64>,
    /// Height of the bar in pixels.
    #[prop(default = 28)]
    height: u32,
) -> impl IntoView {
    let layout = Memo::new(move |_| {
        let segs = segments.get();
        let sum: f64 = segs.iter().map(|s| s.value.max(0.0)).sum();
        let total = total_override.filter(|t| *t > 0.0).unwrap_or(sum).max(1e-6);
        let mut cursor = 0.0_f64;
        let placed: Vec<(StackSegment, f64, f64)> = segs
            .into_iter()
            .map(|seg| {
                let w = (seg.value.max(0.0) / total) * 100.0;
                let start = cursor;
                cursor += w;
                (seg, start, w)
            })
            .collect();
        (placed, total, sum)
    });

    view! {
        <div class="space-y-2">
            <div
                class="relative w-full rounded-md overflow-hidden bg-surface-overlay/40 border border-edge-subtle"
                style=format!("height: {height}px")
            >
                {move || {
                    let (placed, _total, _sum) = layout.get();
                    placed.into_iter().map(|(seg, start, width)| {
                        let color = seg.color;
                        view! {
                            <div
                                class="absolute top-0 bottom-0 transition-all duration-300 group"
                                style=format!(
                                    "left: {start:.3}%; width: {width:.3}%; \
                                     background: linear-gradient(180deg, {color}cc, {color}66); \
                                     box-shadow: inset 0 -1px 0 rgba(0,0,0,0.25), 0 0 8px {color}22"
                                )
                                title=format!("{}: {:.2} ms", seg.label, seg.value)
                            >
                                <div
                                    class="absolute inset-y-0 right-0 w-px"
                                    style="background: rgba(0, 0, 0, 0.35)"
                                />
                            </div>
                        }
                    }).collect_view()
                }}
            </div>
            <div class="grid grid-cols-4 lg:grid-cols-8 gap-1.5">
                {move || {
                    let (placed, _total, _sum) = layout.get();
                    placed.into_iter().map(|(seg, _start, _width)| {
                        let label = seg.label;
                        let color = seg.color;
                        let val = seg.value;
                        view! {
                            <div class="flex items-center gap-1.5 min-w-0">
                                <span
                                    class="w-2 h-2 rounded-sm shrink-0"
                                    style=format!("background: {color}; box-shadow: 0 0 6px {color}55")
                                />
                                <div class="min-w-0 flex-1">
                                    <div class="text-[9px] font-mono uppercase tracking-[0.1em] text-fg-tertiary truncate">
                                        {label}
                                    </div>
                                    <div class="text-[10px] tabular-nums text-fg-secondary">
                                        {format!("{val:.2} ms")}
                                    </div>
                                </div>
                            </div>
                        }
                    }).collect_view()
                }}
            </div>
        </div>
    }
}

// ── Gantt timeline (frame milestones) ────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct TimelineMarker {
    pub label: &'static str,
    pub at_ms: f64,
    pub color: &'static str,
}

/// Horizontal frame timeline showing milestone ticks from frame start (0)
/// to the frame budget. Budget is shown as a dashed end marker.
#[component]
pub fn GanttTimeline(
    #[prop(into)] markers: Signal<Vec<TimelineMarker>>,
    #[prop(into)] budget_ms: Signal<f64>,
    #[prop(into)] actual_ms: Signal<f64>,
) -> impl IntoView {
    const H: f64 = 56.0;

    view! {
        <div class="relative w-full" style=format!("height: {H}px")>
            {move || {
                let budget = budget_ms.get().max(1.0);
                let actual = actual_ms.get().max(0.0);
                let scale = budget.max(actual).max(1.0);
                let actual_pct = (actual / scale * 100.0).min(100.0);
                let budget_pct = (budget / scale * 100.0).min(100.0);
                let ms = markers.get();

                let over_budget = actual > budget * 1.01;
                let actual_color = if over_budget {
                    "var(--color-error-red)"
                } else if actual > budget * 0.85 {
                    "var(--color-electric-yellow)"
                } else {
                    "var(--color-success-green)"
                };

                view! {
                    // Track background
                    <div class="absolute inset-x-0 top-[22px] h-2 rounded-full bg-surface-overlay/60 border border-edge-subtle/60" />

                    // Actual frame duration fill
                    <div
                        class="absolute top-[22px] h-2 rounded-full transition-all duration-300"
                        style=format!(
                            "left: 0; width: {actual_pct:.2}%; \
                             background: linear-gradient(90deg, {actual_color}88, {actual_color}ff); \
                             box-shadow: 0 0 10px {actual_color}66"
                        )
                    />

                    // Budget end marker (dashed)
                    <div
                        class="absolute top-[16px] bottom-[16px] w-px"
                        style=format!(
                            "left: {budget_pct:.2}%; \
                             background: repeating-linear-gradient(to bottom, var(--color-electric-yellow) 0 3px, transparent 3px 6px)"
                        )
                    />
                    <div
                        class="absolute -top-[1px] text-[8px] font-mono text-electric-yellow/80 -translate-x-1/2"
                        style=format!("left: {budget_pct:.2}%")
                    >
                        {format!("budget {budget:.1}ms")}
                    </div>

                    // Milestone ticks
                    {ms.into_iter().map(|m| {
                        let pos_pct = (m.at_ms / scale * 100.0).clamp(0.0, 100.0);
                        view! {
                            <div
                                class="absolute top-[18px] h-[12px] w-[2px] rounded-full"
                                style=format!(
                                    "left: calc({pos_pct:.2}% - 1px); \
                                     background: {color}; \
                                     box-shadow: 0 0 6px {color}",
                                    color = m.color,
                                )
                                title=format!("{}: {:.2} ms", m.label, m.at_ms)
                            />
                            <div
                                class="absolute bottom-[2px] text-[8px] font-mono -translate-x-1/2 whitespace-nowrap"
                                style=format!(
                                    "left: {pos_pct:.2}%; color: {}; opacity: 0.75",
                                    m.color
                                )
                            >
                                {m.label}
                            </div>
                        }
                    }).collect_view()}
                }
            }}
        </div>
    }
}

// ── Distribution bar (percentile markers) ────────────────────────────

/// A horizontal bar with markers for avg / p95 / p99 / max, with an optional
/// budget line overlay. Used for frame time distribution.
#[component]
pub fn DistributionBar(
    #[prop(into)] avg: Signal<f64>,
    #[prop(into)] p95: Signal<f64>,
    #[prop(into)] p99: Signal<f64>,
    #[prop(into)] max: Signal<f64>,
    #[prop(into)] budget: Signal<f64>,
) -> impl IntoView {
    view! {
        <div class="space-y-2">
            {move || {
                let a = avg.get();
                let b95 = p95.get();
                let b99 = p99.get();
                let mx = max.get();
                let bg = budget.get().max(0.1);
                let scale = mx.max(bg * 1.15).max(0.1);
                let pct = |v: f64| (v / scale * 100.0).clamp(0.0, 100.0);
                let budget_pct = pct(bg);

                let marker = |label: &'static str, v: f64, color: &'static str| {
                    let p = pct(v);
                    view! {
                        <div class="relative h-5">
                            <div class="absolute inset-y-[7px] inset-x-0 rounded-full bg-surface-overlay/60 border border-edge-subtle/60" />
                            <div
                                class="absolute inset-y-[7px] left-0 rounded-full transition-all duration-300"
                                style=format!(
                                    "width: {p:.2}%; \
                                     background: linear-gradient(90deg, {color}44, {color}ff); \
                                     box-shadow: 0 0 8px {color}66"
                                )
                            />
                            <div
                                class="absolute inset-y-0 w-px"
                                style=format!(
                                    "left: {budget_pct:.2}%; \
                                     background: repeating-linear-gradient(to bottom, var(--color-electric-yellow) 0 2px, transparent 2px 5px); \
                                     opacity: 0.8"
                                )
                            />
                            <div class="absolute inset-y-0 left-2 flex items-center text-[9px] font-mono uppercase tracking-[0.1em] text-fg-tertiary">
                                {label}
                            </div>
                            <div
                                class="absolute inset-y-0 right-2 flex items-center text-[10px] font-mono tabular-nums"
                                style=format!("color: {color}")
                            >
                                {format!("{v:.2} ms")}
                            </div>
                        </div>
                    }
                };

                view! {
                    {marker("avg", a, "var(--color-success-green)")}
                    {marker("p95", b95, "var(--color-neon-cyan)")}
                    {marker("p99", b99, "var(--color-electric-purple)")}
                    {marker("max", mx, "var(--color-coral)")}
                }
            }}
        </div>
    }
}

// ── Progress ring (compact) ──────────────────────────────────────────

/// Compact progress ring — smaller version of RadialGauge without labels.
/// Used for memory usage, cache hit rate, etc.
#[component]
pub fn ProgressRing(
    #[prop(into)] value: Signal<f64>,
    #[prop(into)] max: Signal<f64>,
    #[prop(into)] label: Signal<String>,
    #[prop(into)] detail: Signal<String>,
    #[prop(default = "var(--color-electric-purple)")] color: &'static str,
) -> impl IntoView {
    const SIZE: f64 = 72.0;
    const STROKE: f64 = 6.0;
    const RADIUS: f64 = (SIZE - STROKE) / 2.0;
    let circumference = 2.0 * std::f64::consts::PI * RADIUS;

    let pct = Memo::new(move |_| {
        let m = max.get();
        if m <= 0.0 {
            0.0
        } else {
            (value.get() / m).clamp(0.0, 1.0)
        }
    });
    let dash_offset = Memo::new(move |_| circumference * (1.0 - pct.get()));

    view! {
        <div class="flex items-center gap-3">
            <div class="relative shrink-0" style=format!("width: {SIZE}px; height: {SIZE}px; overflow: visible")>
                <svg
                    width=SIZE
                    height=SIZE
                    viewBox=format!("0 0 {SIZE} {SIZE}")
                    class="block -rotate-90"
                    style="overflow: visible"
                    aria-hidden="true"
                >
                    <circle
                        cx=SIZE / 2.0
                        cy=SIZE / 2.0
                        r=RADIUS
                        fill="none"
                        stroke="var(--color-edge-subtle)"
                        stroke-width=STROKE
                        stroke-opacity="0.5"
                    />
                    <circle
                        cx=SIZE / 2.0
                        cy=SIZE / 2.0
                        r=RADIUS
                        fill="none"
                        stroke=color
                        stroke-width=STROKE
                        stroke-linecap="round"
                        stroke-dasharray=format!("{circumference:.2}")
                        stroke-dashoffset=move || format!("{:.2}", dash_offset.get())
                        style="transition: stroke-dashoffset 0.4s cubic-bezier(0.4, 0, 0.2, 1); filter: drop-shadow(0 0 4px currentColor)"
                    />
                </svg>
                <div class="absolute inset-0 flex items-center justify-center">
                    <span
                        class="text-[11px] font-mono tabular-nums"
                        style=format!("color: {color}")
                    >
                        {move || format!("{:.0}%", pct.get() * 100.0)}
                    </span>
                </div>
            </div>
            <div class="flex-1 min-w-0">
                <div class="text-[10px] font-mono uppercase tracking-[0.12em] text-fg-tertiary truncate">
                    {move || label.get()}
                </div>
                <div class="text-[12px] tabular-nums text-fg-secondary truncate">
                    {move || detail.get()}
                </div>
            </div>
        </div>
    }
}

// ── Hit-rate progress bar ────────────────────────────────────────────

/// Horizontal mini bar for cache/reuse hit rates. Shows percentage + count.
#[component]
pub fn HitRateBar(
    #[prop(into)] label: Signal<String>,
    #[prop(into)] value: Signal<u32>,
    #[prop(into)] total: Signal<u32>,
    #[prop(default = "var(--color-success-green)")] color: &'static str,
) -> impl IntoView {
    let pct = Memo::new(move |_| {
        let t = f64::from(total.get());
        if t <= 0.0 {
            0.0
        } else {
            (f64::from(value.get()) / t * 100.0).clamp(0.0, 100.0)
        }
    });

    view! {
        <div class="space-y-1">
            <div class="flex items-baseline justify-between gap-2">
                <span class="text-[10px] font-mono uppercase tracking-[0.1em] text-fg-tertiary truncate">
                    {move || label.get()}
                </span>
                <span
                    class="text-[10px] font-mono tabular-nums shrink-0"
                    style=format!("color: {color}")
                >
                    {move || format!("{:.0}%  ({}/{})", pct.get(), value.get(), total.get())}
                </span>
            </div>
            <div class="relative h-1.5 rounded-full bg-surface-overlay/60 border border-edge-subtle/40 overflow-hidden">
                <div
                    class="absolute inset-y-0 left-0 rounded-full transition-all duration-500"
                    style=move || format!(
                        "width: {:.2}%; \
                         background: linear-gradient(90deg, {color}66, {color}ff); \
                         box-shadow: 0 0 6px {color}77",
                        pct.get()
                    )
                />
            </div>
        </div>
    }
}
