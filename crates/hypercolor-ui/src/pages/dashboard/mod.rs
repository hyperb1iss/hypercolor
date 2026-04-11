//! Dashboard page — live preview, favorites, and a rich performance theatre.
//!
//! Layout: a hero row at the top (preview on the left, favorites on the right,
//! with a draggable divider between them) followed by a full-width stats stack
//! below — hero gauges, pipeline visualisations, frame timeline, sparklines,
//! and pacing/memory/throughput panels. Every chart is pure inline SVG driven
//! by reactive Leptos signals fed from the WebSocket metrics stream.

use std::collections::VecDeque;

use leptos::prelude::*;

use leptos_icons::Icon;

use crate::api;
use crate::app::WsContext;
use crate::components::perf_charts::PhaseFrame;
use crate::components::resize_handle::ResizeHandle;
use crate::icons::*;
use crate::preview_telemetry::PreviewTelemetryContext;
use crate::ws::PerformanceMetrics;

mod charts;
mod gauges;
mod header;
mod timeline;

use charts::{DistributionPanel, FavoritesPanel, PipelinePanel, ThroughputPanel};
use gauges::{HeroGauges, MemoryAndDevicesPanel, ReuseRatesPanel};
use header::{PresetDashboardStrip, PreviewCard, StatusSkeleton, StatusStrip};
use timeline::{BackpressureBanner, FrameTimelinePanel, LatestFramePanel, PacingPanel};

// ── Layout tunables ──────────────────────────────────────────────────

const HISTORY_SIZE: usize = 60;
const MIN_PREVIEW_WIDTH: f64 = 280.0;
const MAX_PREVIEW_WIDTH: f64 = 820.0;
const DEFAULT_PREVIEW_WIDTH: f64 = 460.0;
const HERO_ROW_HEIGHT_PX: f64 = 540.0;
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
    phase: PhaseFrame,
}

impl MetricsSample {
    fn from_metrics(m: &PerformanceMetrics, preview_fps: f32) -> Self {
        let t = &m.timeline;
        // Phase durations are derived from the cumulative milestone timeline.
        // Any given milestone may briefly regress by a hair under load, so
        // clamp to zero to keep the waterfall bars well-formed.
        let diff = |later: f64, earlier: f64| (later - earlier).max(0.0) as f32;
        let phase = PhaseFrame {
            input: diff(t.input_done_ms, t.scene_snapshot_done_ms),
            producer: diff(t.producer_done_ms, t.input_done_ms),
            compose: diff(t.composition_done_ms, t.producer_done_ms),
            sample: diff(t.sampling_done_ms, t.composition_done_ms),
            output: diff(t.output_done_ms, t.sampling_done_ms),
            publish: diff(t.publish_done_ms, t.output_done_ms),
            overhead: diff(t.frame_done_ms, t.publish_done_ms),
        };

        Self {
            engine_fps: m.fps.actual,
            frame_time_avg: m.frame_time.avg_ms,
            frame_time_p95: m.frame_time.p95_ms,
            jitter_p95: m.pacing.jitter_p95_ms,
            wake_p95: m.pacing.wake_delay_p95_ms,
            frame_age: m.pacing.frame_age_ms,
            preview_fps,
            ws_bytes_per_sec: m.websocket.bytes_sent_per_sec,
            phase,
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
        history
            .read()
            .iter()
            .map(|s| s.wake_p95)
            .collect::<Vec<_>>()
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
    let series_phase =
        Memo::new(move |_| history.read().iter().map(|s| s.phase).collect::<Vec<_>>());

    view! {
        <div class="flex h-full min-h-0 flex-col overflow-hidden animate-fade-in">
            <header class="shrink-0 glass-subtle border-b border-edge-subtle/15">
                <div class="px-6 py-4 flex items-center gap-5 min-w-0">
                    // ── Title cluster: icon + "Dashboard" ──
                    <div class="flex items-center gap-2.5 shrink-0">
                        <span
                            class="shrink-0 inline-flex items-center justify-center"
                            style="color: rgb(128, 255, 234); \
                                   filter: drop-shadow(0 0 10px rgba(128, 255, 234, 0.55))"
                        >
                            <Icon icon=LuActivity width="20px" height="20px" />
                        </span>
                        <h1
                            class="leading-none logo-gradient-text whitespace-nowrap"
                            style="font-family:'Orbitron',sans-serif; font-weight:900; \
                                   font-size:22px; letter-spacing:-0.01em; \
                                   background-image:linear-gradient(105deg,#80ffea 0%,#d4eaff 50%,#50fa7b 100%)"
                        >
                            "Dashboard"
                        </h1>
                    </div>

                    // Vertical divider between title and tagline.
                    <div class="w-px h-6 bg-edge-subtle/30 shrink-0" />

                    // ── Tagline (truncates before pills) ──
                    <p class="text-[12px] text-fg-tertiary/75 truncate min-w-0 flex-1">
                        "Live render preview, system health, and frame pipeline telemetry."
                    </p>

                    // ── Status pills (right-aligned, shrink-0) ──
                    <Suspense fallback=move || view! { <StatusSkeleton /> }>
                        {move || status_resource.get().map(|result| {
                            match result {
                                Ok(status) => view! { <StatusStrip status=status metrics=ws.metrics /> }.into_any(),
                                Err(e) => view! {
                                    <div class="text-[11px] text-status-error shrink-0">
                                        "Failed to connect: " {e}
                                    </div>
                                }.into_any(),
                            }
                        })}
                    </Suspense>
                </div>
            </header>

            <div class="flex-1 min-h-0 overflow-y-auto">
                <div class="p-6 pt-4 flex flex-col gap-4 min-h-full">
                    // ── Hero row: preview on the left, favorites on the
                    // right, draggable splitter between them. Fixed height
                    // so the stats section below stays visible on load. ──
                    <div
                        class="flex items-stretch gap-0 shrink-0"
                        style=move || format!("height: {HERO_ROW_HEIGHT_PX}px")
                    >
                        <div
                            class="shrink-0 h-full flex flex-col gap-2 min-h-0"
                            style=move || format!("width: {}px", preview_width.get())
                        >
                            <div class="flex-1 min-h-0">
                                <PreviewCard />
                            </div>
                            <PresetDashboardStrip />
                        </div>

                        <ResizeHandle
                            on_drag_start=on_drag_start
                            on_drag=on_drag
                            on_drag_end=on_drag_end
                        />

                        <div class="flex-1 min-w-0 h-full flex flex-col min-h-0">
                            <FavoritesPanel />
                        </div>
                    </div>

                    // ── Stats stack: full page width under the hero row. ──
                    <section class="flex flex-col gap-4 min-w-0">
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

                        <FrameTimelinePanel
                            metrics=ws.metrics
                            phase_history=Signal::derive(move || series_phase.get())
                        />

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
        </div>
    }
}

fn read_stored_width() -> Option<f64> {
    crate::storage::get_parsed(PREVIEW_WIDTH_STORAGE_KEY)
}

fn persist_width(width: f64) {
    crate::storage::set(PREVIEW_WIDTH_STORAGE_KEY, &width.to_string());
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
