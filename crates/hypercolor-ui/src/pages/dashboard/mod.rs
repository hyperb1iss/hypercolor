//! Dashboard page — live preview, favorites, and a rich performance theatre.
//!
//! Layout: a hero row at the top (preview on the left, favorites on the right,
//! with a draggable divider between them) followed by a full-width stats stack
//! below — hero gauges, pipeline visualisations, frame timeline, sparklines,
//! and pacing/memory/throughput panels. Every chart is pure inline SVG driven
//! by reactive Leptos signals fed from the WebSocket metrics stream.

use std::collections::VecDeque;

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::{UseEventListenerOptions, use_event_listener_with_options};
use wasm_bindgen::JsCast;

use crate::api;
use crate::app::WsContext;
use crate::components::perf_charts::PhaseFrame;
use crate::components::preview_cabinet::PreviewCabinet;
use crate::components::resize_handle::ResizeHandle;
use crate::icons::*;
use crate::preview_telemetry::PreviewTelemetryContext;
use crate::ws::PerformanceMetrics;

mod charts;
mod gauges;
mod header;
mod layout;
mod panel_frame;
mod timeline;

use charts::{DistributionPanel, FavoritesPanel, PipelinePanel, ThroughputPanel};
use gauges::{HeroGauges, MemoryAndDevicesPanel, ReuseRatesPanel};
use header::{StatusSkeleton, StatusStrip};
use layout::{DashboardLayout, PanelId};
use panel_frame::PanelFrame;
use timeline::{BackpressureBanner, FrameTimelinePanel, LatestFramePanel, PacingPanel};

// ── Layout tunables ──────────────────────────────────────────────────

const HISTORY_SIZE: usize = 60;
const MIN_PREVIEW_WIDTH: f64 = 280.0;
const MAX_PREVIEW_WIDTH: f64 = 820.0;
const DEFAULT_PREVIEW_WIDTH: f64 = 460.0;
const HERO_ROW_HEIGHT_PX: f64 = 540.0;
const PREVIEW_WIDTH_STORAGE_KEY: &str = "hc-dashboard-preview-width";
const DASHBOARD_PREVIEW_FPS_CAP: u32 = 60;
const DASHBOARD_PREVIEW_MIN_REQUEST_WIDTH: f64 = 640.0;
const DASHBOARD_PREVIEW_MAX_REQUEST_WIDTH: f64 = 1280.0;
const DASHBOARD_PREVIEW_REQUEST_QUANTUM: f64 = 64.0;

fn dashboard_preview_request_width(preview_width_px: f64) -> u32 {
    let device_pixel_ratio = web_sys::window().map_or(1.0, |window| window.device_pixel_ratio());
    let scaled_width = (preview_width_px * device_pixel_ratio).clamp(
        DASHBOARD_PREVIEW_MIN_REQUEST_WIDTH,
        DASHBOARD_PREVIEW_MAX_REQUEST_WIDTH,
    );
    let quantized_width = (scaled_width / DASHBOARD_PREVIEW_REQUEST_QUANTUM).ceil()
        * DASHBOARD_PREVIEW_REQUEST_QUANTUM;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        quantized_width as u32
    }
}

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

    // Match the effects page cadence, but only request enough preview pixels
    // to fill the dashboard cabinet on the current display.
    Effect::new(move |_| {
        ws.set_preview_cap.set(DASHBOARD_PREVIEW_FPS_CAP);
        ws.set_preview_width_cap
            .set(dashboard_preview_request_width(preview_width.get()));
    });
    on_cleanup(move || {
        ws.set_preview_cap.set(crate::ws::DEFAULT_PREVIEW_FPS_CAP);
        ws.set_preview_width_cap.set(0);
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

    // ── Layout state: panel order, widths, visibility ───────────────
    //
    // Loaded from localStorage on mount, written back on every
    // mutation. The drag_source signal is shared by every panel frame
    // so dragstart on one frame flows through to every frame's drop
    // target highlighting.
    let dash_layout = RwSignal::new(DashboardLayout::load());
    let drag_source: RwSignal<Option<usize>> = RwSignal::new(None);
    let (layout_menu_open, set_layout_menu_open) = signal(false);

    let on_panel_drop = Callback::new(move |(from, to): (usize, usize)| {
        dash_layout.update(|l| {
            l.move_panel(from, to);
            l.save();
        });
        drag_source.set(None);
    });
    let on_panel_cycle_width = Callback::new(move |id: PanelId| {
        dash_layout.update(|l| {
            l.cycle_width(id);
            l.save();
        });
    });
    let on_panel_hide = Callback::new(move |id: PanelId| {
        dash_layout.update(|l| {
            l.set_visible(id, false);
            l.save();
        });
    });
    let on_panel_show = Callback::new(move |id: PanelId| {
        dash_layout.update(|l| {
            l.set_visible(id, true);
            l.save();
        });
    });
    let on_layout_reset = Callback::new(move |()| {
        dash_layout.set(DashboardLayout::default_layout());
        dash_layout.with_untracked(|l| l.save());
        set_layout_menu_open.set(false);
    });

    view! {
        <div class="flex h-full min-h-0 flex-col overflow-hidden animate-fade-in">
            <Show when=move || layout_menu_open.get()>
                <LayoutMenuDismissHandler set_open=set_layout_menu_open />
            </Show>
            // `relative z-30` lifts the header into its own stacking
            // context above the scroll container below, so the layout
            // gear menu can pop down past the header edge and float
            // over the hero row instead of being clipped behind it.
            <header class="relative z-30 shrink-0 glass-subtle border-b border-edge-default">
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

                    // Layout gear — opens the panel visibility / reset menu.
                    // A small coral dot badges the icon when one or more
                    // panels are currently hidden from the grid, so the
                    // gear is the only way to get them back.
                    <div class="layout-menu-anchor relative shrink-0">
                        <button
                            type="button"
                            class="relative p-1.5 rounded-lg text-fg-tertiary hover:text-fg-primary \
                                   hover:bg-surface-hover/40 transition-all"
                            class=("text-electric-purple", move || layout_menu_open.get())
                            title="Dashboard layout"
                            on:click=move |_| set_layout_menu_open.update(|v| *v = !*v)
                        >
                            <Icon icon=LuLayoutDashboard width="15px" height="15px" />
                            {move || dash_layout.read().has_hidden().then(|| view! {
                                <span
                                    class="absolute top-1 right-1 w-1.5 h-1.5 rounded-full"
                                    style="background: var(--color-coral); \
                                           box-shadow: 0 0 6px rgba(255, 106, 193, 0.9)"
                                />
                            })}
                        </button>
                        <Show when=move || layout_menu_open.get()>
                            <LayoutMenu
                                layout=dash_layout
                                on_show=on_panel_show
                                on_hide=on_panel_hide
                                on_reset=on_layout_reset
                            />
                        </Show>
                    </div>
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
                            class="shrink-0 h-full"
                            style=move || format!("width: {}px", preview_width.get())
                        >
                            <PreviewCabinet report_telemetry=true fill_height=true />
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

                    // ── Stats grid: layout-driven, drag-to-reorder, per-panel
                    // width and visibility. Panels below `xl` always
                    // collapse to full width; above `xl` each panel honors
                    // its stored `PanelWidth`. ──
                    <section class="min-w-0">
                        <div class="grid grid-cols-6 gap-4 min-w-0">
                            {move || {
                                let panels: Vec<_> = dash_layout
                                    .read()
                                    .panels
                                    .iter()
                                    .cloned()
                                    .enumerate()
                                    .filter(|(_, p)| p.visible)
                                    .collect();
                                panels
                                    .into_iter()
                                    .map(|(idx, config)| {
                                        let panel_view = match config.id {
                                            PanelId::HeroGauges => view! {
                                                <HeroGauges
                                                    metrics=ws.metrics
                                                    preview_fps=ws.preview_fps
                                                    preview_target_fps=ws.preview_target_fps
                                                    preview_present=preview_telemetry.presenter
                                                    engine_fps_series=Signal::derive(move || series_engine_fps.get())
                                                    frame_time_series=Signal::derive(move || series_frame_avg.get())
                                                    preview_fps_series=Signal::derive(move || series_preview_fps.get())
                                                />
                                            }.into_any(),
                                            PanelId::Pipeline => view! {
                                                <PipelinePanel metrics=ws.metrics />
                                            }.into_any(),
                                            PanelId::FrameTimeline => view! {
                                                <FrameTimelinePanel
                                                    metrics=ws.metrics
                                                    phase_history=Signal::derive(move || series_phase.get())
                                                />
                                            }.into_any(),
                                            PanelId::Distribution => view! {
                                                <DistributionPanel metrics=ws.metrics />
                                            }.into_any(),
                                            PanelId::Pacing => view! {
                                                <PacingPanel
                                                    metrics=ws.metrics
                                                    jitter_series=Signal::derive(move || series_jitter.get())
                                                    wake_series=Signal::derive(move || series_wake.get())
                                                    frame_age_series=Signal::derive(move || series_frame_age.get())
                                                    frame_p95_series=Signal::derive(move || series_frame_p95.get())
                                                />
                                            }.into_any(),
                                            PanelId::ReuseRates => view! {
                                                <ReuseRatesPanel metrics=ws.metrics />
                                            }.into_any(),
                                            PanelId::MemoryAndDevices => view! {
                                                <MemoryAndDevicesPanel metrics=ws.metrics />
                                            }.into_any(),
                                            PanelId::Throughput => view! {
                                                <ThroughputPanel
                                                    metrics=ws.metrics
                                                    ws_bytes_series=Signal::derive(move || series_ws_bytes.get())
                                                />
                                            }.into_any(),
                                            PanelId::LatestFrame => view! {
                                                <LatestFramePanel metrics=ws.metrics />
                                            }.into_any(),
                                        };
                                        view! {
                                            <PanelFrame
                                                panel_id=config.id
                                                width=config.width
                                                index=idx
                                                drag_source=drag_source
                                                on_drop=on_panel_drop
                                                on_cycle_width=on_panel_cycle_width
                                                on_hide=on_panel_hide
                                            >
                                                {panel_view}
                                            </PanelFrame>
                                        }
                                    })
                                    .collect_view()
                            }}
                        </div>

                        {move || ws.backpressure_notice.get().map(|notice| view! {
                            <div class="mt-4">
                                <BackpressureBanner notice=notice />
                            </div>
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

// ── Layout menu ──────────────────────────────────────────────────────

/// Dashboard header popover — list of every panel with visibility
/// toggles, plus a reset-to-default button. The only way to un-hide a
/// panel once it's been dismissed from the floating control bar.
#[component]
fn LayoutMenu(
    layout: RwSignal<DashboardLayout>,
    on_show: Callback<PanelId>,
    on_hide: Callback<PanelId>,
    on_reset: Callback<()>,
) -> impl IntoView {
    view! {
        <div
            class="absolute right-0 top-full mt-2 w-64 rounded-xl glass-dense border \
                   border-edge-default dropdown-glow animate-slide-down z-50 overflow-hidden"
            style="background: linear-gradient(180deg, \
                   rgba(18, 14, 28, 0.95) 0%, \
                   rgba(10, 8, 20, 0.96) 100%)"
            on:mousedown=|ev: ev::MouseEvent| ev.stop_propagation()
        >
            <div class="px-3 pt-3 pb-2 flex items-center gap-2">
                <Icon
                    icon=LuLayoutDashboard
                    width="12px"
                    height="12px"
                    style="color: var(--color-electric-purple)"
                />
                <span class="text-[10px] font-mono uppercase tracking-[0.16em] font-semibold text-electric-purple">
                    "Dashboard panels"
                </span>
            </div>

            <div class="px-2 pb-2 flex flex-col gap-0.5">
                {move || {
                    layout
                        .read()
                        .panels
                        .iter()
                        .cloned()
                        .map(|config| {
                            let id = config.id;
                            let visible = config.visible;
                            view! {
                                <button
                                    type="button"
                                    class="flex items-center gap-2.5 px-2 py-1.5 rounded-md text-left \
                                           text-[11px] transition-colors hover:bg-surface-hover/40"
                                    on:click=move |_| {
                                        if visible {
                                            on_hide.run(id);
                                        } else {
                                            on_show.run(id);
                                        }
                                    }
                                >
                                    <span
                                        class="shrink-0 inline-flex items-center justify-center w-4 h-4 rounded \
                                               transition-all"
                                        class=("text-electric-purple", move || visible)
                                        class=("text-fg-tertiary/40", move || !visible)
                                        style=if visible {
                                            "background: rgba(225, 53, 255, 0.12); \
                                             border: 1px solid rgba(225, 53, 255, 0.35)"
                                        } else {
                                            "background: rgba(255, 255, 255, 0.02); \
                                             border: 1px solid rgba(255, 255, 255, 0.08)"
                                        }
                                    >
                                        {if visible {
                                            view! { <Icon icon=LuEye width="9px" height="9px" /> }.into_any()
                                        } else {
                                            view! { <Icon icon=LuEyeOff width="9px" height="9px" /> }.into_any()
                                        }}
                                    </span>
                                    <span
                                        class="flex-1 min-w-0 truncate"
                                        class=("text-fg-primary", move || visible)
                                        class=("text-fg-tertiary", move || !visible)
                                    >
                                        {id.label()}
                                    </span>
                                    <span class="text-[9px] font-mono uppercase tracking-wider text-fg-tertiary/50">
                                        {config.width.label()}
                                    </span>
                                </button>
                            }
                        })
                        .collect_view()
                }}
            </div>

            <div class="h-px bg-edge-subtle/40" />

            <button
                type="button"
                class="w-full px-3 py-2.5 flex items-center gap-2 text-[11px] text-fg-tertiary \
                       hover:text-electric-purple hover:bg-electric-purple/5 transition-colors"
                on:click=move |_| on_reset.run(())
            >
                <Icon icon=LuRotateCcw width="11px" height="11px" />
                <span>"Reset to default layout"</span>
            </button>
        </div>
    }
}

/// One-time document-level mousedown listener that closes the dashboard
/// layout menu when the user clicks outside its anchor. Mirrors the
/// pattern used in `preset_panel::install_dropdown_outside_handler`.
fn install_layout_menu_outside_handler(set_open: WriteSignal<bool>) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };

    let _ = use_event_listener_with_options(
        doc,
        ev::mousedown,
        move |ev: leptos::ev::MouseEvent| {
            let inside = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .map(|el| el.closest(".layout-menu-anchor").ok().flatten().is_some())
                .unwrap_or(false);
            if !inside {
                set_open.set(false);
            }
        },
        UseEventListenerOptions::default().capture(true),
    );
}

#[component]
fn LayoutMenuDismissHandler(set_open: WriteSignal<bool>) -> impl IntoView {
    install_layout_menu_outside_handler(set_open);
    view! {}
}
