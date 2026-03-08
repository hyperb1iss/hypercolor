//! Dashboard page — system overview with mini preview and quick-switch.

use leptos::prelude::*;

use crate::api::{self, EffectSummary, SystemStatus};
use crate::app::{EffectsContext, WsContext};
use crate::components::canvas_preview::CanvasPreview;
use crate::toasts;
use crate::ws::{BackpressureNotice, PerformanceMetrics};

/// Dashboard landing page.
#[component]
pub fn DashboardPage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let fx = expect_context::<EffectsContext>();
    let status_resource = LocalResource::new(api::fetch_status);

    let canvas_frame = Signal::derive(move || ws.canvas_frame.get());
    let preview_fps = Signal::derive(move || ws.preview_fps.get());
    let metrics = Signal::derive(move || ws.metrics.get());
    let backpressure = Signal::derive(move || ws.backpressure_notice.get());

    view! {
        <div class="space-y-6 max-w-5xl animate-fade-in">
            // Hero
            <div>
                <h1 class="text-lg font-medium text-fg-primary mb-0.5">"Dashboard"</h1>
                <p class="text-[13px] text-fg-secondary">"Hypercolor lighting engine overview"</p>
            </div>

            // Status cards
            <Suspense fallback=move || view! { <StatusSkeleton /> }>
                {move || status_resource.get().map(|result| {
                    match result {
                        Ok(status) => {
                            let on_change = {
                                let status_resource = status_resource;
                                Callback::new(move |brightness: u8| {
                                    let status_resource = status_resource;
                                    leptos::task::spawn_local(async move {
                                        match api::set_global_brightness(brightness).await {
                                            Ok(_) => status_resource.refetch(),
                                            Err(error) => {
                                                toasts::toast_error(&format!(
                                                    "Global brightness update failed: {error}"
                                                ));
                                            }
                                        }
                                    });
                                })
                            };

                            view! {
                                <div class="space-y-3">
                                    <StatusCards status=status.clone() />
                                    <GlobalBrightnessCard
                                        brightness=status.global_brightness
                                        on_change=on_change
                                    />
                                </div>
                            }
                            .into_any()
                        }
                        Err(e) => view! {
                            <div class="text-sm text-status-error bg-status-error/[0.05] border border-status-error/10 rounded-lg px-4 py-3">
                                "Failed to connect: " {e}
                            </div>
                        }.into_any(),
                    }
                })}
            </Suspense>

            <PerformancePanel
                preview_fps=preview_fps
                preview_target_fps=ws.preview_target_fps
                metrics=metrics
                backpressure=backpressure
            />

            // Main: preview + quick switch
            <div class="grid grid-cols-1 lg:grid-cols-2 gap-5">
                // Live preview
                <div class="rounded-xl bg-surface-overlay/60 border border-edge-subtle overflow-hidden">
                    <div class="px-4 py-3 border-b border-edge-subtle flex items-center justify-between">
                        <h2 class="text-[14px] font-medium text-fg-secondary">"Live Preview"</h2>
                        {move || ws.active_effect.get().map(|name| {
                            view! {
                                <div class="flex items-center gap-1.5">
                                    <div class="w-1.5 h-1.5 rounded-full bg-accent animate-pulse" />
                                    <span class="text-[11px] text-fg-secondary font-mono">{name}</span>
                                </div>
                            }
                        })}
                    </div>
                    <div class="p-3">
                        <CanvasPreview
                            frame=canvas_frame
                            fps=preview_fps
                            show_fps=true
                            fps_target=ws.preview_target_fps
                        />
                    </div>
                </div>

                // Quick switch
                <div class="rounded-xl bg-surface-overlay/60 border border-edge-subtle">
                    <div class="px-4 py-3 border-b border-edge-subtle">
                        <h2 class="text-[14px] font-medium text-fg-secondary">"Quick Switch"</h2>
                    </div>
                    <div class="p-3">
                        <Suspense fallback=move || view! {
                            <div class="text-xs text-fg-tertiary py-4 text-center">"Loading effects..."</div>
                        }>
                            {move || {
                                let runnable: Vec<_> = fx
                                    .effects_index
                                    .get()
                                    .into_iter()
                                    .map(|entry| entry.effect)
                                    .filter(|effect| effect.runnable)
                                    .take(20)
                                    .collect();
                                view! { <QuickSwitchGrid effects=runnable /> }.into_any()
                            }}
                        </Suspense>
                    </div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn PerformancePanel(
    #[prop(into)] preview_fps: Signal<f32>,
    #[prop(into)] preview_target_fps: Signal<u32>,
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
    #[prop(into)] backpressure: Signal<Option<BackpressureNotice>>,
) -> impl IntoView {
    let stage_chip = |label: &'static str, value: Signal<f64>| {
        view! {
            <div class="rounded-lg border border-edge-subtle bg-surface-overlay/30 px-3 py-2">
                <div class="text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary mb-1">
                    {label}
                </div>
                <div class="text-sm tabular-nums text-fg-secondary">
                    {move || format!("{:.2} ms", value.get())}
                </div>
            </div>
        }
    };

    let engine_text = Signal::derive(move || {
        metrics
            .get()
            .map(|metrics| format!("{:.1}/{} fps", metrics.fps.actual, metrics.fps.target))
            .unwrap_or_else(|| "Waiting...".to_string())
    });
    let engine_hint = Signal::derive(move || {
        metrics
            .get()
            .map(|metrics| format!("{} budget misses", metrics.fps.dropped))
            .unwrap_or_else(|| "render loop".to_string())
    });
    let frame_time_text = Signal::derive(move || {
        metrics
            .get()
            .map(|metrics| format!("{:.2} ms avg", metrics.frame_time.avg_ms))
            .unwrap_or_else(|| "Waiting...".to_string())
    });
    let frame_time_hint = Signal::derive(move || {
        metrics
            .get()
            .map(|metrics| format!("p95 {:.2} ms", metrics.frame_time.p95_ms))
            .unwrap_or_else(|| "collecting samples".to_string())
    });
    let preview_text = Signal::derive(move || {
        format!("{:.1}/{} fps", preview_fps.get(), preview_target_fps.get())
    });
    let preview_hint = Signal::derive(move || {
        if preview_target_fps.get() <= 15 {
            "debug preview cap".to_string()
        } else {
            "canvas stream delivery".to_string()
        }
    });
    let websocket_text = Signal::derive(move || {
        metrics
            .get()
            .map(|metrics| format_bytes_per_sec(metrics.websocket.bytes_sent_per_sec))
            .unwrap_or_else(|| "Waiting...".to_string())
    });
    let websocket_hint = Signal::derive(move || {
        metrics
            .get()
            .map(|metrics| format!("{} client(s)", metrics.websocket.client_count))
            .unwrap_or_else(|| "metrics channel".to_string())
    });
    let device_output_text = Signal::derive(move || {
        metrics
            .get()
            .map(|metrics| metrics.devices.output_errors.to_string())
            .unwrap_or_else(|| "0".to_string())
    });
    let device_output_hint = Signal::derive(move || {
        metrics
            .get()
            .map(|metrics| {
                format!(
                    "{} devices, {} LEDs",
                    metrics.devices.connected, metrics.devices.total_leds
                )
            })
            .unwrap_or_else(|| "device output".to_string())
    });

    let input_stage = Signal::derive(move || {
        metrics
            .get()
            .map(|metrics| metrics.stages.input_sampling_ms)
            .unwrap_or_default()
    });
    let render_stage = Signal::derive(move || {
        metrics
            .get()
            .map(|metrics| metrics.stages.effect_rendering_ms)
            .unwrap_or_default()
    });
    let sample_stage = Signal::derive(move || {
        metrics
            .get()
            .map(|metrics| metrics.stages.spatial_sampling_ms)
            .unwrap_or_default()
    });
    let push_stage = Signal::derive(move || {
        metrics
            .get()
            .map(|metrics| metrics.stages.device_output_ms)
            .unwrap_or_default()
    });
    let publish_stage = Signal::derive(move || {
        metrics
            .get()
            .map(|metrics| metrics.stages.event_bus_ms)
            .unwrap_or_default()
    });

    let metric_card = |label: &'static str, value: Signal<String>, hint: Signal<String>| {
        view! {
            <div class="rounded-lg border border-edge-subtle bg-surface-overlay/30 px-4 py-3">
                <div class="text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary mb-1.5">
                    {label}
                </div>
                <div class="text-lg font-medium tabular-nums text-fg-primary">{move || value.get()}</div>
                <div class="text-[11px] text-fg-tertiary mt-1">{move || hint.get()}</div>
            </div>
        }
    };

    view! {
        <div class="rounded-xl bg-surface-overlay/60 border border-edge-subtle overflow-hidden">
            <div class="px-4 py-3 border-b border-edge-subtle flex items-center justify-between gap-3">
                <div>
                    <h2 class="text-[14px] font-medium text-fg-secondary">"Performance"</h2>
                    <p class="text-[11px] text-fg-tertiary">"Separate engine timing from preview delivery and websocket transport."</p>
                </div>
                <div class="text-[10px] font-mono text-fg-tertiary rounded-full border border-edge-subtle bg-surface-overlay/30 px-2.5 py-1">
                    {move || {
                        metrics
                            .get()
                            .map(|metrics| format!("budget misses {}", metrics.fps.dropped))
                            .unwrap_or_else(|| "metrics warming up".to_string())
                    }}
                </div>
            </div>

            <div class="p-4 space-y-4">
                <div class="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-4 gap-3">
                    {metric_card("Engine", engine_text, engine_hint)}
                    {metric_card("Preview", preview_text, preview_hint)}
                    {metric_card("Frame Time", frame_time_text, frame_time_hint)}
                    {metric_card("WebSocket", websocket_text, websocket_hint)}
                </div>

                <div class="grid grid-cols-2 lg:grid-cols-5 gap-2">
                    {stage_chip("Input", input_stage)}
                    {stage_chip("Render", render_stage)}
                    {stage_chip("Sample", sample_stage)}
                    {stage_chip("Queue", push_stage)}
                    {stage_chip("Publish", publish_stage)}
                </div>

                <div class="rounded-lg border border-edge-subtle bg-surface-overlay/20 px-3 py-2 text-[12px] text-fg-secondary">
                    <span class="font-mono text-fg-primary mr-2">"Output Errors"</span>
                    <span class="tabular-nums">{move || device_output_text.get()}</span>
                    <span class="text-fg-tertiary ml-2">{move || device_output_hint.get()}</span>
                </div>

                {move || backpressure.get().map(|notice| {
                    view! {
                        <div class="rounded-lg border border-electric-yellow/20 bg-electric-yellow/[0.06] px-3 py-2 text-[12px] text-electric-yellow">
                            <span class="font-mono uppercase tracking-[0.14em] mr-2">"Backpressure"</span>
                            {format!(
                                "{} dropped on {}. {} -> {} fps",
                                notice.dropped_frames,
                                notice.channel,
                                notice.recommendation.replace('_', " "),
                                notice.suggested_fps
                            )}
                        </div>
                    }
                })}
            </div>
        </div>
    }
}

/// Status metric cards.
#[component]
fn StatusCards(status: SystemStatus) -> impl IntoView {
    let cards = vec![
        (
            "Status",
            if status.running { "Running" } else { "Stopped" }.to_string(),
            status.running,
            "electric-purple",
        ),
        (
            "Uptime",
            format_uptime(status.uptime_seconds),
            true,
            "neon-cyan",
        ),
        (
            "Devices",
            status.device_count.to_string(),
            status.device_count > 0,
            "coral",
        ),
        (
            "Effects",
            status.effect_count.to_string(),
            true,
            "success-green",
        ),
    ];

    view! {
        <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
            {cards.into_iter().enumerate().map(|(i, (label, value, healthy, _accent))| {
                let delay_class = format!("stagger-{}", i + 1);
                view! {
                    <div class=format!("rounded-xl bg-surface-overlay/60 border border-edge-subtle px-4 py-3 animate-fade-in-up {delay_class}")>
                        <div class="text-[9px] font-mono uppercase tracking-[0.15em] text-fg-tertiary mb-1.5">{label}</div>
                        <div
                            class="text-lg font-medium tabular-nums"
                            class=("text-fg-primary", healthy)
                            class=("text-fg-secondary", !healthy)
                        >
                            {value}
                        </div>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

/// Quick-switch effect grid.
#[component]
fn GlobalBrightnessCard(brightness: u8, on_change: Callback<u8>) -> impl IntoView {
    view! {
        <div class="rounded-xl bg-surface-overlay/60 border border-edge-subtle px-4 py-3">
            <div class="flex items-center justify-between gap-3 mb-2">
                <div>
                    <div class="text-[9px] font-mono uppercase tracking-[0.15em] text-fg-tertiary mb-1">
                        "Global Brightness"
                    </div>
                    <div class="text-[12px] text-fg-secondary">
                        "Master output scale across every device."
                    </div>
                </div>
                <div class="text-lg font-medium tabular-nums text-fg-primary">
                    {format!("{brightness}%")}
                </div>
            </div>
            <input
                type="range"
                min="0"
                max="100"
                step="1"
                class="w-full h-1 rounded-full appearance-none cursor-pointer"
                style="accent-color: rgb(225, 53, 255); background: rgba(139, 133, 160, 0.15)"
                prop:value=brightness.to_string()
                on:change=move |ev| {
                    let target = event_target_value(&ev);
                    if let Ok(brightness) = target.parse::<u8>() {
                        on_change.run(brightness);
                    }
                }
            />
        </div>
    }
}

/// Quick-switch effect grid.
#[component]
fn QuickSwitchGrid(effects: Vec<EffectSummary>) -> impl IntoView {
    let fx = expect_context::<EffectsContext>();

    if effects.is_empty() {
        return view! {
            <p class="text-xs text-fg-tertiary py-4 text-center">"No runnable effects found"</p>
        }
        .into_any();
    }

    view! {
        <div class="grid grid-cols-2 gap-1.5 max-h-[400px] overflow-y-auto pr-1">
            {effects.into_iter().enumerate().map(|(i, effect)| {
                let id = effect.id.clone();
                let name = effect.name.clone();
                let category = effect.category.clone();
                let delay = format!("animation-delay: {}ms", i * 25);
                view! {
                    <button
                        class="text-left px-3 py-2 rounded-lg bg-surface-overlay/20 border border-edge-subtle
                               hover:bg-accent-subtle hover:border-accent-muted card-hover btn-press group
                               animate-fade-in-up"
                        style=delay
                        on:click=move |_| fx.apply_effect(id.clone())
                    >
                        <div class="text-[12px] text-fg-secondary truncate group-hover:text-fg-primary transition-colors">{name}</div>
                        <div class="text-[10px] text-fg-tertiary capitalize">{category}</div>
                    </button>
                }
            }).collect_view()}
        </div>
    }
    .into_any()
}

/// Skeleton for status loading.
#[component]
fn StatusSkeleton() -> impl IntoView {
    view! {
        <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
            {(0..4).map(|_| view! {
                <div class="rounded-xl bg-surface-overlay/40 border border-edge-subtle px-4 py-3 animate-pulse">
                    <div class="h-2.5 w-12 bg-surface-overlay/40 rounded mb-2" />
                    <div class="h-5 w-16 bg-surface-overlay/40 rounded" />
                </div>
            }).collect_view()}
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
