//! Dashboard page — live preview, favorites, and a rich performance theatre.
//!
//! Layout: a resizable sidebar (preview + favorites) next to a data-heavy main
//! column with hero gauges, pipeline visualisations, frame timeline, sparklines,
//! and pacing/memory/throughput panels. Every chart is pure inline SVG driven
//! by reactive Leptos signals fed from the WebSocket metrics stream.

use std::collections::VecDeque;

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::{self, EffectSummary, SystemStatus};
use crate::app::{EffectsContext, WsContext};
use crate::components::canvas_preview::CanvasPreview;
use crate::components::perf_charts::{
    DistributionBar, GanttTimeline, HitRateBar, ProgressRing, RadialGauge, Sparkline, StackSegment,
    StackedBar, TimelineMarker,
};
use crate::components::resize_handle::ResizeHandle;
use crate::icons::*;
use crate::preview_telemetry::{PreviewPresenterTelemetry, PreviewTelemetryContext};
use crate::color;
use crate::style_utils::{category_accent_rgb, category_style};
use crate::ws::{BackpressureNotice, PerformanceMetrics};

// ── Layout tunables ──────────────────────────────────────────────────

const HISTORY_SIZE: usize = 60;
const MIN_PREVIEW_WIDTH: f64 = 280.0;
const MAX_PREVIEW_WIDTH: f64 = 720.0;
const DEFAULT_PREVIEW_WIDTH: f64 = 420.0;
const PREVIEW_WIDTH_STORAGE_KEY: &str = "hc-dashboard-preview-width";
const DASHBOARD_PREVIEW_FPS_CAP: u32 = 60;

// ── Rolling metrics history ──────────────────────────────────────────

/// Compact snapshot of the telemetry values we want to graph over time.
/// Captured every metrics tick and stashed in a bounded ring buffer so the
/// sparklines always have recent history to draw.
#[derive(Clone, Copy, Default)]
struct MetricsSample {
    engine_fps: f64,
    frame_time_avg: f64,
    frame_time_p95: f64,
    jitter_p95: f64,
    wake_p95: f64,
    frame_age: f64,
    preview_fps: f32,
    ws_bytes_per_sec: f64,
}

impl MetricsSample {
    fn from_metrics(m: &PerformanceMetrics, preview_fps: f32) -> Self {
        Self {
            engine_fps: m.fps.actual,
            frame_time_avg: m.frame_time.avg_ms,
            frame_time_p95: m.frame_time.p95_ms,
            jitter_p95: m.pacing.jitter_p95_ms,
            wake_p95: m.pacing.wake_delay_p95_ms,
            frame_age: m.pacing.frame_age_ms,
            preview_fps,
            ws_bytes_per_sec: m.websocket.bytes_sent_per_sec,
        }
    }
}

// ── Dashboard root ───────────────────────────────────────────────────

/// Dashboard landing page.
#[component]
pub fn DashboardPage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let preview_telemetry = expect_context::<PreviewTelemetryContext>();
    let status_resource = LocalResource::new(api::fetch_status);

    // Match the effects page cadence so the preview isn't judder-capped to 30
    // when you switch over from /effects. Restores on leave.
    Effect::new(move |_| {
        ws.set_preview_cap.set(DASHBOARD_PREVIEW_FPS_CAP);
    });
    on_cleanup(move || ws.set_preview_cap.set(crate::ws::DEFAULT_PREVIEW_FPS_CAP));

    // Resizable preview column — persisted across reloads.
    let (preview_width, set_preview_width) = signal(
        read_stored_width()
            .unwrap_or(DEFAULT_PREVIEW_WIDTH)
            .clamp(MIN_PREVIEW_WIDTH, MAX_PREVIEW_WIDTH),
    );
    let drag_start_width = StoredValue::new(0.0_f64);

    let on_drag_start = Callback::new(move |()| {
        drag_start_width.set_value(preview_width.get_untracked());
        set_resizing_body(true);
    });
    let on_drag = Callback::new(move |delta_x: f64| {
        let base = drag_start_width.get_value();
        let new_w = (base + delta_x).clamp(MIN_PREVIEW_WIDTH, MAX_PREVIEW_WIDTH);
        set_preview_width.set(new_w);
    });
    let on_drag_end = Callback::new(move |()| {
        set_resizing_body(false);
        persist_width(preview_width.get_untracked());
    });

    // Rolling history — one signal driven by the metrics stream.
    let history = RwSignal::new(VecDeque::<MetricsSample>::with_capacity(HISTORY_SIZE));
    let metrics_signal = ws.metrics;
    let preview_fps_signal = ws.preview_fps;
    Effect::new(move |_| {
        if let Some(m) = metrics_signal.get() {
            let sample = MetricsSample::from_metrics(&m, preview_fps_signal.get_untracked());
            history.update(|h| {
                if h.len() >= HISTORY_SIZE {
                    h.pop_front();
                }
                h.push_back(sample);
            });
        }
    });

    // Series extractors — each chart subscribes to only the field it needs.
    let series_engine_fps = Memo::new(move |_| {
        history
            .read()
            .iter()
            .map(|s| s.engine_fps)
            .collect::<Vec<_>>()
    });
    let series_frame_avg = Memo::new(move |_| {
        history
            .read()
            .iter()
            .map(|s| s.frame_time_avg)
            .collect::<Vec<_>>()
    });
    let series_frame_p95 = Memo::new(move |_| {
        history
            .read()
            .iter()
            .map(|s| s.frame_time_p95)
            .collect::<Vec<_>>()
    });
    let series_jitter = Memo::new(move |_| {
        history
            .read()
            .iter()
            .map(|s| s.jitter_p95)
            .collect::<Vec<_>>()
    });
    let series_wake = Memo::new(move |_| {
        history.read().iter().map(|s| s.wake_p95).collect::<Vec<_>>()
    });
    let series_frame_age = Memo::new(move |_| {
        history
            .read()
            .iter()
            .map(|s| s.frame_age)
            .collect::<Vec<_>>()
    });
    let series_preview_fps = Memo::new(move |_| {
        history
            .read()
            .iter()
            .map(|s| f64::from(s.preview_fps))
            .collect::<Vec<_>>()
    });
    let series_ws_bytes = Memo::new(move |_| {
        history
            .read()
            .iter()
            .map(|s| s.ws_bytes_per_sec)
            .collect::<Vec<_>>()
    });

    view! {
        <div class="h-full overflow-y-auto animate-fade-in">
            <div class="p-5 flex gap-0 items-stretch min-h-full">
                // ── Sticky sidebar column (preview + favorites + resize handle).
                // Wrapping both the aside and the handle keeps them pinned
                // together while the data column scrolls underneath. ──
                <div
                    class="sticky top-0 self-start shrink-0 flex items-stretch"
                    style="height: calc(100vh - 2.5rem)"
                >
                    <aside
                        class="flex flex-col gap-4 min-h-0"
                        style=move || format!("width: {}px", preview_width.get())
                    >
                        <PreviewCard />
                        <FavoritesPanel />
                        <ResizeHint />
                    </aside>

                    // Resize handle — visible grip between sidebar and data.
                    <ResizeHandle
                        on_drag_start=on_drag_start
                        on_drag=on_drag
                        on_drag_end=on_drag_end
                    />
                </div>

                // ── Right column: all the juicy data ──
                <section class="flex-1 min-w-0 flex flex-col gap-4">
                    // Page title
                    <div class="flex items-center gap-2 shrink-0">
                        <span style="color: #80ffea; filter: drop-shadow(0 0 8px rgba(128, 255, 234, 0.75))">
                            <Icon icon=LuActivity width="20px" height="20px" />
                        </span>
                        <h1
                            class="leading-none logo-gradient-text"
                            style="font-family:'Orbitron',sans-serif; font-weight:900; font-size:22px; \
                                   letter-spacing:-0.01em; \
                                   background-image:linear-gradient(105deg,#80ffea 0%,#d4eaff 50%,#50fa7b 100%)"
                        >
                            "Dashboard"
                        </h1>
                    </div>

                    <Suspense fallback=move || view! { <StatusSkeleton /> }>
                        {move || status_resource.get().map(|result| {
                            match result {
                                Ok(status) => view! { <StatusStrip status=status metrics=ws.metrics /> }.into_any(),
                                Err(e) => view! {
                                    <div class="text-sm text-status-error bg-status-error/[0.05] border border-status-error/10 rounded-lg px-4 py-3">
                                        "Failed to connect: " {e}
                                    </div>
                                }.into_any(),
                            }
                        })}
                    </Suspense>

                    <HeroGauges
                        metrics=ws.metrics
                        preview_fps=ws.preview_fps
                        preview_target_fps=ws.preview_target_fps
                        preview_present=preview_telemetry.presenter
                        engine_fps_series=Signal::derive(move || series_engine_fps.get())
                        frame_time_series=Signal::derive(move || series_frame_avg.get())
                        preview_fps_series=Signal::derive(move || series_preview_fps.get())
                    />

                    <PipelinePanel metrics=ws.metrics />

                    <FrameTimelinePanel metrics=ws.metrics />

                    <div class="grid grid-cols-1 xl:grid-cols-2 gap-4">
                        <DistributionPanel metrics=ws.metrics />
                        <PacingPanel
                            metrics=ws.metrics
                            jitter_series=Signal::derive(move || series_jitter.get())
                            wake_series=Signal::derive(move || series_wake.get())
                            frame_age_series=Signal::derive(move || series_frame_age.get())
                            frame_p95_series=Signal::derive(move || series_frame_p95.get())
                        />
                    </div>

                    <div class="grid grid-cols-1 xl:grid-cols-2 gap-4">
                        <ReuseRatesPanel metrics=ws.metrics />
                        <MemoryAndDevicesPanel metrics=ws.metrics />
                    </div>

                    <ThroughputPanel
                        metrics=ws.metrics
                        ws_bytes_series=Signal::derive(move || series_ws_bytes.get())
                    />

                    <LatestFramePanel metrics=ws.metrics />

                    {move || ws.backpressure_notice.get().map(|notice| view! {
                        <BackpressureBanner notice=notice />
                    })}
                </section>
            </div>
        </div>
    }
}

/// Tiny affordance at the bottom of the sidebar explaining that the split is
/// draggable. Pure discoverability — users kept missing the 2px handle.
#[component]
fn ResizeHint() -> impl IntoView {
    view! {
        <div class="shrink-0 flex items-center justify-center gap-1.5 py-1.5 text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary/50 hover:text-electric-purple/70 transition-colors">
            <Icon icon=LuChevronLeft width="10px" height="10px" />
            <span>"drag edge to resize"</span>
            <Icon icon=LuChevronRight width="10px" height="10px" />
        </div>
    }
}

// ── Cinematic preview card ────────────────────────────────────────────

/// Cinematic preview with scrim overlay showing active effect info, matching
/// the effects page's treatment. Canvas as background, metadata overlaid at
/// the bottom with category-accent-tinted text.
#[component]
fn PreviewCard() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let fx = expect_context::<EffectsContext>();

    let accent_rgb = Signal::derive(move || {
        category_accent_rgb(&fx.active_effect_category.get()).to_string()
    });
    let title_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.86, 0.65));
    let body_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.78, 0.22));
    let meta_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.68, 0.65));

    let effect_meta = Memo::new(move |_| {
        fx.active_effect_id.get().and_then(|id| {
            fx.effects_index.with(|effects| {
                effects
                    .iter()
                    .find(|e| e.effect.id == id)
                    .map(|e| e.effect.clone())
            })
        })
    });

    view! {
        <div
            class="relative rounded-xl overflow-hidden border border-edge-subtle bg-black edge-glow"
            style:--glow-rgb=move || accent_rgb.get()
            style:border-top=move || format!("2px solid rgba({}, 0.45)", accent_rgb.get())
        >
            <CanvasPreview
                frame=ws.canvas_frame
                fps=ws.preview_fps
                show_fps=false
                fps_target=ws.preview_target_fps
                report_presenter_telemetry=true
            />

            // Scrim — transparent at top, fades dark at bottom for legible overlay text
            <div
                class="absolute inset-0 pointer-events-none"
                style="background: linear-gradient(180deg, \
                       rgba(0, 0, 0, 0) 0%, \
                       rgba(0, 0, 0, 0) 40%, \
                       rgba(0, 0, 0, 0.78) 78%, \
                       rgba(0, 0, 0, 0.95) 100%)"
            />

            // Top accent wash — colored highlight along the top edge
            <div
                class="absolute top-0 left-0 right-0 h-px pointer-events-none"
                style=move || format!(
                    "background: linear-gradient(90deg, transparent 0%, rgba({0}, 0.8) 50%, transparent 100%); \
                     box-shadow: 0 0 14px rgba({0}, 0.55)",
                    accent_rgb.get()
                )
            />

            // Info overlay — effect name, description, category + audio badge
            <div class="absolute left-0 right-0 bottom-0 px-3.5 pb-3 pt-8 pointer-events-none">
                {move || {
                    let name = fx.active_effect_name.get();
                    let meta = effect_meta.get();

                    name.map(|effect_name| {
                        let description = meta.as_ref().map(|m| m.description.clone()).unwrap_or_default();
                        let category = meta.as_ref().map(|m| m.category.clone()).unwrap_or_default();
                        let audio_reactive = meta.as_ref().is_some_and(|m| m.audio_reactive);
                        let source = meta.as_ref().map(|m| m.source.clone()).unwrap_or_default();
                        let is_html = source == "html";
                        let show_source = source != "native";

                        view! {
                            <h3
                                class="text-[14px] font-semibold line-clamp-1 leading-tight \
                                       drop-shadow-[0_2px_8px_rgba(0,0,0,0.85)] mb-0.5"
                                style:color=move || format!("rgb({})", title_tint.get())
                            >
                                {effect_name}
                            </h3>

                            {(!description.is_empty()).then(|| view! {
                                <p
                                    class="text-[10px] line-clamp-2 leading-relaxed mb-2 \
                                           drop-shadow-[0_1px_4px_rgba(0,0,0,0.85)]"
                                    style:color=move || format!("rgba({}, 0.88)", body_tint.get())
                                >
                                    {description}
                                </p>
                            })}

                            <div class="flex items-center justify-between gap-2">
                                <div class="flex items-center gap-1.5 min-w-0">
                                    <div
                                        class="w-1.5 h-1.5 rounded-full shrink-0 dot-alive"
                                        style:background=move || format!("rgb({})", accent_rgb.get())
                                        style:box-shadow=move || format!("0 0 6px rgba({}, 0.75)", accent_rgb.get())
                                    />
                                    <span
                                        class="text-[10px] font-mono uppercase tracking-wider capitalize truncate \
                                               drop-shadow-[0_1px_3px_rgba(0,0,0,0.85)]"
                                        style:color=move || format!("rgb({})", meta_tint.get())
                                    >
                                        {category}
                                    </span>
                                </div>
                                <div class="flex items-center gap-1.5 shrink-0">
                                    {show_source.then(|| {
                                        let icon = if is_html { LuGlobe } else { LuCode };
                                        view! {
                                            <span
                                                class="inline-flex items-center text-[9px] font-mono px-1.5 py-0.5 \
                                                       rounded-full bg-white/5 backdrop-blur-sm"
                                                style:color=move || format!("rgba({}, 0.85)", meta_tint.get())
                                            >
                                                <Icon icon=icon width="11px" height="11px" />
                                            </span>
                                        }
                                    })}
                                    {audio_reactive.then(|| view! {
                                        <span
                                            class="inline-flex items-center text-coral/90 px-1.5 py-0.5 \
                                                   rounded-full bg-coral/15 backdrop-blur-sm"
                                            title="Audio-reactive"
                                        >
                                            <Icon icon=LuAudioLines width="11px" height="11px" />
                                        </span>
                                    })}
                                </div>
                            </div>
                        }
                    })
                }}
            </div>
        </div>
    }
}

// ── Status strip ─────────────────────────────────────────────────────

#[component]
fn StatusStrip(
    status: SystemStatus,
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
) -> impl IntoView {
    let running = status.running;
    let uptime = format_uptime(status.uptime_seconds);
    let device_count = status.device_count;
    let effect_count = status.effect_count;

    let ws_clients = Memo::new(move |_| {
        metrics.get().map_or(0, |m| m.websocket.client_count)
    });

    view! {
        <div class="px-4 py-3 flex flex-wrap items-center gap-5 animate-fade-in-up border-b border-edge-subtle/50">
            <StatusPill
                label="Status"
                value=if running { "Running" } else { "Stopped" }
                color=if running { "var(--color-success-green)" } else { "var(--color-error-red)" }
                pulsing=running
            />
            <div class="w-px h-6 bg-edge-subtle/30" />
            <StatusPill
                label="Uptime"
                value=uptime.as_str()
                color="var(--color-neon-cyan)"
                pulsing=false
            />
            <div class="w-px h-6 bg-edge-subtle/30" />
            <StatusPill
                label="Devices"
                value=format!("{device_count}")
                color="var(--color-coral)"
                pulsing=false
            />
            <div class="w-px h-6 bg-edge-subtle/30" />
            <StatusPill
                label="Effects"
                value=format!("{effect_count}")
                color="var(--color-electric-purple)"
                pulsing=false
            />
            <div class="w-px h-6 bg-edge-subtle/30" />
            <StatusPillDynamic
                label="WS Clients"
                value=Signal::derive(move || ws_clients.get().to_string())
                color="var(--color-electric-yellow)"
            />
        </div>
    }
}

#[component]
fn StatusPill(
    label: &'static str,
    #[prop(into)] value: String,
    color: &'static str,
    pulsing: bool,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2.5">
            <div
                class="w-2 h-2 rounded-full shrink-0"
                class=("animate-pulse", pulsing)
                style=format!("background: {color}; box-shadow: 0 0 8px {color}aa")
            />
            <div>
                <div class="text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary">{label}</div>
                <div
                    class="text-[14px] font-semibold tabular-nums leading-none mt-0.5"
                    style=format!("color: {color}")
                >
                    {value}
                </div>
            </div>
        </div>
    }
}

#[component]
fn StatusPillDynamic(
    label: &'static str,
    #[prop(into)] value: Signal<String>,
    color: &'static str,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2.5">
            <div
                class="w-2 h-2 rounded-full shrink-0"
                style=format!("background: {color}; box-shadow: 0 0 8px {color}aa")
            />
            <div>
                <div class="text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary">{label}</div>
                <div
                    class="text-[14px] font-semibold tabular-nums leading-none mt-0.5"
                    style=format!("color: {color}")
                >
                    {move || value.get()}
                </div>
            </div>
        </div>
    }
}

// ── Hero gauges: Engine FPS / Frame Time / Preview FPS ───────────────

#[component]
fn HeroGauges(
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
    #[prop(into)] preview_fps: Signal<f32>,
    #[prop(into)] preview_target_fps: Signal<u32>,
    #[prop(into)] preview_present: Signal<PreviewPresenterTelemetry>,
    #[prop(into)] engine_fps_series: Signal<Vec<f64>>,
    #[prop(into)] frame_time_series: Signal<Vec<f64>>,
    #[prop(into)] preview_fps_series: Signal<Vec<f64>>,
) -> impl IntoView {
    // Engine FPS gauge values
    let engine_value = Memo::new(move |_| metrics.get().map_or(0.0, |m| m.fps.actual));
    let engine_max = Memo::new(move |_| metrics.get().map_or(60.0, |m| f64::from(m.fps.target).max(1.0)));
    let engine_primary = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("{:.1}", m.fps.actual))
            .unwrap_or_else(|| "—".into())
    });
    let engine_secondary = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("/ {} fps", m.fps.target))
            .unwrap_or_else(|| "waiting".into())
    });

    // Frame time gauge — inverted: lower is better. Fill = budget - actual.
    let frame_value = Memo::new(move |_| metrics.get().map_or(0.0, |m| m.frame_time.avg_ms));
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
        metrics
            .get()
            .map(|m| format!("{:.2}", m.frame_time.avg_ms))
            .unwrap_or_else(|| "—".into())
    });
    let frame_secondary = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("/ {:.1} ms", if m.fps.target > 0 { 1000.0 / f64::from(m.fps.target) } else { 33.33 }))
            .unwrap_or_else(|| "ms".into())
    });

    // Preview gauge
    let preview_value = Memo::new(move |_| {
        let present = preview_present.get().present_fps;
        if present > 0.0 {
            f64::from(present)
        } else {
            f64::from(preview_fps.get())
        }
    });
    let preview_max = Memo::new(move |_| f64::from(preview_target_fps.get()).max(1.0));
    let preview_primary = Memo::new(move |_| format!("{:.1}", preview_value.get()));
    let preview_secondary = Memo::new(move |_| {
        let target = preview_target_fps.get();
        let present = preview_present.get();
        let mode = present.runtime_mode.unwrap_or("pending");
        let arrival = present.arrival_to_present_ms;
        if arrival > 0.0 {
            format!("/ {target} fps · {mode} · {arrival:.1} ms")
        } else {
            format!("/ {target} fps · {mode}")
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

// ── Pipeline breakdown ───────────────────────────────────────────────

#[component]
fn PipelinePanel(#[prop(into)] metrics: Signal<Option<PerformanceMetrics>>) -> impl IntoView {
    let segments = Memo::new(move |_| {
        let Some(m) = metrics.get() else {
            return Vec::<StackSegment>::new();
        };
        let s = &m.stages;
        vec![
            StackSegment { label: "Input", value: s.input_sampling_ms, color: "#80ffea" },
            StackSegment { label: "Producer", value: s.producer_rendering_ms, color: "#e135ff" },
            StackSegment { label: "Compose", value: s.composition_ms, color: "#ff6ac1" },
            StackSegment { label: "Sample", value: s.spatial_sampling_ms, color: "#ff99ff" },
            StackSegment { label: "Output", value: s.device_output_ms, color: "#f1fa8c" },
            StackSegment { label: "Post", value: s.preview_postprocess_ms, color: "#82aaff" },
            StackSegment { label: "Publish", value: s.event_bus_ms, color: "#50fa7b" },
            StackSegment { label: "Overhead", value: s.coordination_overhead_ms, color: "#808090" },
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
                format!("Σ {total:.2} ms · budget {:.1} ms", if m.fps.target > 0 { 1000.0 / f64::from(m.fps.target) } else { 33.33 })
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

// ── Frame timeline (Gantt) ───────────────────────────────────────────

#[component]
fn FrameTimelinePanel(#[prop(into)] metrics: Signal<Option<PerformanceMetrics>>) -> impl IntoView {
    let markers = Memo::new(move |_| {
        let Some(m) = metrics.get() else {
            return Vec::<TimelineMarker>::new();
        };
        let t = &m.timeline;
        vec![
            TimelineMarker { label: "wake", at_ms: t.wake_late_ms.max(0.0), color: "#f1fa8c" },
            TimelineMarker { label: "snap", at_ms: t.scene_snapshot_done_ms, color: "#82aaff" },
            TimelineMarker { label: "input", at_ms: t.input_done_ms, color: "#80ffea" },
            TimelineMarker { label: "prod", at_ms: t.producer_done_ms, color: "#e135ff" },
            TimelineMarker { label: "comp", at_ms: t.composition_done_ms, color: "#ff6ac1" },
            TimelineMarker { label: "samp", at_ms: t.sampling_done_ms, color: "#ff99ff" },
            TimelineMarker { label: "out", at_ms: t.output_done_ms, color: "#f1fa8c" },
            TimelineMarker { label: "pub", at_ms: t.publish_done_ms, color: "#50fa7b" },
        ]
    });
    let budget = Memo::new(move |_| metrics.get().map_or(33.33, |m| m.timeline.budget_ms.max(0.1)));
    let actual = Memo::new(move |_| metrics.get().map_or(0.0, |m| m.timeline.frame_done_ms));
    let token_text = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| {
                format!(
                    "frame #{} · {} layer{} · {} group{}",
                    m.timeline.frame_token,
                    m.timeline.logical_layer_count,
                    if m.timeline.logical_layer_count == 1 { "" } else { "s" },
                    m.timeline.render_group_count,
                    if m.timeline.render_group_count == 1 { "" } else { "s" },
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
            <div class="p-4">
                <GanttTimeline
                    markers=Signal::derive(move || markers.get())
                    budget_ms=budget
                    actual_ms=actual
                />
            </div>
        </div>
    }
}

// ── Distribution panel (frame time percentiles) ─────────────────────

#[component]
fn DistributionPanel(#[prop(into)] metrics: Signal<Option<PerformanceMetrics>>) -> impl IntoView {
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

// ── Pacing panel (sparklines) ────────────────────────────────────────

#[component]
fn PacingPanel(
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
    #[prop(into)] jitter_series: Signal<Vec<f64>>,
    #[prop(into)] wake_series: Signal<Vec<f64>>,
    #[prop(into)] frame_age_series: Signal<Vec<f64>>,
    #[prop(into)] frame_p95_series: Signal<Vec<f64>>,
) -> impl IntoView {
    let jitter_label = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("p95 {:.2} ms · max {:.2} ms", m.pacing.jitter_p95_ms, m.pacing.jitter_max_ms))
            .unwrap_or_else(|| "—".into())
    });
    let wake_label = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("p95 {:.2} ms · max {:.2} ms", m.pacing.wake_delay_p95_ms, m.pacing.wake_delay_max_ms))
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
                <span class="text-[10px] font-mono uppercase tracking-[0.12em] text-fg-tertiary">{label}</span>
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

// ── Reuse rates ──────────────────────────────────────────────────────

#[component]
fn ReuseRatesPanel(#[prop(into)] metrics: Signal<Option<PerformanceMetrics>>) -> impl IntoView {
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
fn MemoryAndDevicesPanel(
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

// ── Throughput panel ─────────────────────────────────────────────────

#[component]
fn ThroughputPanel(
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
            .map(|m| format!("{} client{}", m.websocket.client_count, if m.websocket.client_count == 1 { "" } else { "s" }))
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

// ── Latest frame detail ──────────────────────────────────────────────

#[component]
fn LatestFramePanel(#[prop(into)] metrics: Signal<Option<PerformanceMetrics>>) -> impl IntoView {
    let line = Memo::new(move |_| {
        metrics
            .get()
            .and_then(|m| {
                if m.timeline.frame_token == 0 {
                    return None;
                }
                Some(format!(
                    "#{} · wake {:.2} · snap {:.2} · input {:.2} · prod {:.2} · comp {:.2} · samp {:.2} · out {:.2} · pub {:.2} · frame {:.2}",
                    m.timeline.frame_token,
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
                    <span class="text-[10px] font-mono uppercase tracking-[0.14em] text-fg-tertiary">"Latest Frame"</span>
                </div>
            </div>
            <div class="text-[11px] font-mono text-fg-secondary/90 break-all">
                {move || line.get()}
            </div>
        </div>
    }
}

#[component]
fn BackpressureBanner(notice: BackpressureNotice) -> impl IntoView {
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

// ── Favorites (compact) ──────────────────────────────────────────────

#[component]
fn FavoritesPanel() -> impl IntoView {
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

// ── Skeleton & helpers ───────────────────────────────────────────────

#[component]
fn StatusSkeleton() -> impl IntoView {
    view! {
        <div class="rounded-xl bg-surface-overlay/40 border border-edge-subtle px-4 py-3 animate-pulse">
            <div class="flex gap-5">
                {(0..5).map(|_| view! {
                    <div class="flex items-center gap-2.5">
                        <div class="w-2 h-2 rounded-full bg-surface-overlay/60" />
                        <div>
                            <div class="h-2 w-10 bg-surface-overlay/50 rounded mb-1" />
                            <div class="h-3 w-14 bg-surface-overlay/50 rounded" />
                        </div>
                    </div>
                }).collect_view()}
            </div>
        </div>
    }
}

fn format_uptime(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
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

fn read_stored_width() -> Option<f64> {
    let storage = web_sys::window()?.local_storage().ok()??;
    let value = storage.get_item(PREVIEW_WIDTH_STORAGE_KEY).ok()??;
    value.parse::<f64>().ok()
}

fn persist_width(width: f64) {
    let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return;
    };
    let _ = storage.set_item(PREVIEW_WIDTH_STORAGE_KEY, &width.to_string());
}

fn set_resizing_body(active: bool) {
    let Some(body) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.body())
    else {
        return;
    };
    if active {
        let _ = body.class_list().add_1("resizing");
    } else {
        let _ = body.class_list().remove_1("resizing");
    }
}
