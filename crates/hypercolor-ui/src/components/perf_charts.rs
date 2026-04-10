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
    const PAD_X: f64 = 5.0;
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
        let draw_w = W - PAD_X * 2.0;
        let project_x = |i: usize| -> f64 {
            PAD_X + (i as f64 / (n - 1).max(1) as f64) * draw_w
        };
        let project_y = |v: f64| -> f64 {
            let t = (v - lo) / range;
            H - PAD_Y - t * (H - PAD_Y * 2.0)
        };

        let mut line = String::with_capacity(n * 12);
        for (i, v) in vs.iter().enumerate() {
            let x = project_x(i);
            let y = project_y(*v);
            if i == 0 {
                line.push_str(&format!("M{x:.1},{y:.1}"));
            } else {
                line.push_str(&format!(" L{x:.1},{y:.1}"));
            }
        }
        let mut area = line.clone();
        area.push_str(&format!(" L{:.1},{:.1}", project_x(n - 1), H));
        area.push_str(&format!(" L{PAD_X},{H} Z"));

        let baseline_y = baseline.map(project_y);
        let last_point = vs.last().copied().map(|v| (project_x(n - 1), project_y(v)));

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
                <div
                    class="absolute inset-0 flex flex-col items-center justify-center pointer-events-none"
                    style="overflow: visible"
                >
                    <div
                        class="text-[22px] font-semibold tabular-nums leading-none"
                        style=format!("color: {color}")
                    >
                        {move || primary.get()}
                    </div>
                    <div class="mt-1 text-[9px] font-mono text-fg-tertiary uppercase tracking-[0.08em] whitespace-nowrap">
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

// ── Phase waterfall (rolling history of frame phase durations) ──────

/// A single frame's phase-breakdown durations in milliseconds. Captured
/// each metrics tick and pushed into a ring buffer so the waterfall has
/// history to render.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhaseFrame {
    pub input: f32,
    pub producer: f32,
    pub compose: f32,
    pub sample: f32,
    pub output: f32,
    pub publish: f32,
    pub overhead: f32,
}

impl PhaseFrame {
    #[inline]
    pub fn total(self) -> f32 {
        self.input
            + self.producer
            + self.compose
            + self.sample
            + self.output
            + self.publish
            + self.overhead
    }
}

// Ordering here defines bottom→top stacking in the waterfall columns and
// is aligned with the Pipeline Breakdown panel's legend colors.
const PHASE_COLORS: [&str; 7] = [
    "#80ffea", // input    — cyan
    "#e135ff", // producer — electric purple
    "#ff6ac1", // compose  — coral
    "#ff99ff", // sample   — pink
    "#f1fa8c", // output   — yellow
    "#50fa7b", // publish  — green
    "#8b85a0", // overhead — muted lavender
];

/// Rolling stacked-column waterfall of frame phase timings. Newest sample
/// sits on the right and the view scrolls left as frames arrive. Auto-
/// scales to the tallest column in view, with a budget line that only
/// renders when it's within range.
#[component]
pub fn PhaseWaterfall(
    #[prop(into)] frames: Signal<Vec<PhaseFrame>>,
    #[prop(into)] budget_ms: Signal<f64>,
    #[prop(default = 128)] height: u32,
) -> impl IntoView {
    view! {
        <div class="space-y-2">
            {move || {
                let fs = frames.get();
                let budget = budget_ms.get().max(0.1) as f32;

                let max_total = fs.iter().map(|f| f.total()).fold(0.0_f32, f32::max);
                // Scale to data with a 0.2ms floor so sub-millisecond frames
                // still read on screen. Extend to include budget only when
                // we're actually pushing it.
                let data_scale = (max_total * 1.25).max(0.2);
                let scale = if max_total >= budget * 0.5 {
                    data_scale.max(budget * 1.05)
                } else {
                    data_scale
                };
                let budget_in_range = budget <= scale * 1.02;
                let budget_y_pct = if budget_in_range {
                    100.0 - (budget / scale * 100.0).clamp(0.0, 100.0)
                } else {
                    0.0
                };

                let n = fs.len();
                let last_idx = n.saturating_sub(1);

                view! {
                    <div
                        class="relative w-full rounded-md bg-surface-overlay/30 border border-edge-subtle/40 overflow-hidden"
                        style=format!("height: {height}px")
                    >
                        // Subtle horizontal grid — 25 / 50 / 75% reference lines
                        <div class="absolute inset-x-0 top-1/4 h-px bg-edge-subtle/20 pointer-events-none" />
                        <div class="absolute inset-x-0 top-1/2 h-px bg-edge-subtle/20 pointer-events-none" />
                        <div class="absolute inset-x-0 top-3/4 h-px bg-edge-subtle/20 pointer-events-none" />

                        // Budget reference line
                        {budget_in_range.then(|| view! {
                            <div
                                class="absolute inset-x-0 h-px pointer-events-none"
                                style=format!(
                                    "top: {budget_y_pct:.2}%; \
                                     background: repeating-linear-gradient(to right, var(--color-electric-yellow) 0 4px, transparent 4px 8px); \
                                     opacity: 0.55"
                                )
                            />
                        })}

                        // Columns — flex so they share width evenly
                        <div class="absolute inset-0 flex items-end gap-[1px] px-1 pb-0.5">
                            {fs.into_iter().enumerate().map(|(i, f)| {
                                let total = f.total().max(1e-6);
                                let col_h_pct = (f.total() / scale * 100.0).clamp(0.0, 100.0);
                                let is_latest = i == last_idx;
                                // Older frames fade subtly so the eye tracks newest data
                                let age_opacity = 0.45 + 0.55 * (i as f32 / n.max(1) as f32);
                                let glow = if is_latest {
                                    "filter: drop-shadow(0 0 6px rgba(128, 255, 234, 0.55));"
                                } else {
                                    ""
                                };

                                let segs: [f32; 7] = [
                                    f.input,
                                    f.producer,
                                    f.compose,
                                    f.sample,
                                    f.output,
                                    f.publish,
                                    f.overhead,
                                ];

                                let mut cursor = 0.0_f32;
                                let segments_view = segs.iter().enumerate()
                                    .filter(|(_, v)| **v > 0.0)
                                    .map(|(phase_idx, v)| {
                                        let seg_h_pct = (v / total * 100.0).clamp(0.0, 100.0);
                                        let bottom_pct = cursor;
                                        cursor += seg_h_pct;
                                        let color = PHASE_COLORS[phase_idx];
                                        view! {
                                            <div
                                                class="absolute inset-x-0"
                                                style=format!(
                                                    "bottom: {bottom_pct:.2}%; \
                                                     height: {seg_h_pct:.2}%; \
                                                     background: linear-gradient(180deg, {color}ee, {color}cc); \
                                                     box-shadow: inset 0 -1px 0 rgba(0,0,0,0.25)"
                                                )
                                            />
                                        }
                                    }).collect_view();

                                view! {
                                    <div
                                        class="flex-1 relative min-w-0 transition-all duration-300 ease-out"
                                        style=format!(
                                            "height: {col_h_pct:.2}%; opacity: {age_opacity:.2}; {glow}"
                                        )
                                        title=format!("frame {}: {:.2} ms total", i, f.total())
                                    >
                                        {segments_view}
                                    </div>
                                }
                            }).collect_view()}
                        </div>

                        // Scale label (top-left) + budget annotation (top-right)
                        <div class="absolute top-1 left-2 right-2 flex items-center justify-between pointer-events-none text-[9px] font-mono tabular-nums">
                            <span class="text-fg-tertiary/50">
                                {format!("{scale:.2} ms")}
                            </span>
                            {if budget_in_range {
                                view! {
                                    <span class="text-electric-yellow/60">
                                        {format!("budget {budget:.1}ms")}
                                    </span>
                                }.into_any()
                            } else {
                                view! {
                                    <span class="text-electric-yellow/60">
                                        {format!("budget {budget:.1}ms · headroom")}
                                    </span>
                                }.into_any()
                            }}
                        </div>
                    </div>

                    // Phase legend row — dots + labels
                    <div class="flex items-center flex-wrap gap-x-3 gap-y-1 text-[9px] font-mono uppercase tracking-[0.08em] text-fg-tertiary">
                        {["input", "producer", "compose", "sample", "output", "publish", "overhead"]
                            .iter()
                            .enumerate()
                            .map(|(i, label)| {
                                let color = PHASE_COLORS[i];
                                view! {
                                    <div class="flex items-center gap-1">
                                        <span
                                            class="w-1.5 h-1.5 rounded-sm"
                                            style=format!("background: {color}; box-shadow: 0 0 4px {color}")
                                        />
                                        <span>{*label}</span>
                                    </div>
                                }
                            }).collect_view()}
                    </div>
                }
            }}
        </div>
    }
}

// ── Distribution bar (percentile markers) ────────────────────────────

/// Horizontal percentile bars for avg / p95 / p99 / max. Auto-scales to
/// the largest percentile so sub-millisecond data remains readable. The
/// budget line is only drawn when it falls inside the zoomed range —
/// otherwise it's shown as an off-chart annotation at the top.
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

                // Scale to the largest percentile with headroom. Floor at 0.2ms
                // so sub-millisecond data doesn't hug the left edge.
                let data_max = mx.max(b99).max(b95).max(a);
                let data_scale = (data_max * 1.25).max(0.2);
                // Only extend to include budget if we're actually pushing it.
                let scale = if data_max >= bg * 0.5 {
                    data_scale.max(bg * 1.05)
                } else {
                    data_scale
                };
                let budget_in_range = bg <= scale * 1.02;
                let pct = move |v: f64| (v / scale * 100.0).clamp(0.0, 100.0);
                let budget_pct = pct(bg);

                let marker = move |label: &'static str, v: f64, color: &'static str| {
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
                            {budget_in_range.then(|| view! {
                                <div
                                    class="absolute inset-y-0 w-px"
                                    style=format!(
                                        "left: {budget_pct:.2}%; \
                                         background: repeating-linear-gradient(to bottom, var(--color-electric-yellow) 0 2px, transparent 2px 5px); \
                                         opacity: 0.8"
                                    )
                                />
                            })}
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
                    <div class="flex items-center justify-between text-[9px] font-mono tabular-nums px-0.5 pt-0.5">
                        <span class="text-fg-tertiary/60">"0 ms"</span>
                        {if budget_in_range {
                            view! { <span class="text-fg-tertiary/60">{format!("{scale:.2} ms")}</span> }.into_any()
                        } else {
                            view! {
                                <span class="text-electric-yellow/70">
                                    {format!("scale {scale:.2}ms · budget {bg:.1}ms (headroom)")}
                                </span>
                            }.into_any()
                        }}
                    </div>
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
