//! Dashboard page — preview + favorites + performance stats.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::{self, EffectSummary, SystemStatus};
use crate::app::{EffectsContext, WsContext};
use crate::components::canvas_preview::CanvasPreview;
use crate::icons::*;
use crate::ws::{BackpressureNotice, PerformanceMetrics};

/// Dashboard landing page.
#[component]
pub fn DashboardPage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let status_resource = LocalResource::new(api::fetch_status);

    view! {
        <div class="space-y-5 max-w-6xl animate-fade-in">
            <h1 class="text-lg font-medium text-fg-primary">"Dashboard"</h1>
            // Top row: preview + favorites side by side
            <div class="grid grid-cols-1 lg:grid-cols-5 gap-5">
                // Live preview — takes 3/5 width
                <div class="lg:col-span-3 rounded-xl bg-surface-overlay/60 border border-edge-subtle overflow-hidden">
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
                            frame=ws.canvas_frame
                            fps=ws.preview_fps
                            show_fps=true
                            fps_target=ws.preview_target_fps
                        />
                    </div>
                </div>

                // Favorites — takes 2/5 width
                <div class="lg:col-span-2 rounded-xl bg-surface-overlay/60 border border-edge-subtle flex flex-col">
                    <FavoritesPanel />
                </div>
            </div>

            // Status cards row
            <Suspense fallback=move || view! { <StatusSkeleton /> }>
                {move || status_resource.get().map(|result| {
                    match result {
                        Ok(status) => view! { <StatusCards status=status /> }.into_any(),
                        Err(e) => view! {
                            <div class="text-sm text-status-error bg-status-error/[0.05] border border-status-error/10 rounded-lg px-4 py-3">
                                "Failed to connect: " {e}
                            </div>
                        }.into_any(),
                    }
                })}
            </Suspense>

            // Performance stats
            <PerformancePanel
                preview_fps=ws.preview_fps
                preview_target_fps=ws.preview_target_fps
                metrics=ws.metrics
                backpressure=ws.backpressure_notice
            />
        </div>
    }
}

// ── Favorites panel ──────────────────────────────────────────────────

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
        <div class="px-4 py-3 border-b border-edge-subtle flex items-center justify-between">
            <div class="flex items-center gap-2">
                <Icon icon=LuHeart width="14px" height="14px" style="color: var(--coral)" />
                <h2 class="text-[14px] font-medium text-fg-secondary">"Favorites"</h2>
            </div>
            <span class="text-[10px] font-mono text-fg-tertiary rounded-full border border-edge-subtle bg-surface-overlay/30 px-2 py-0.5">
                {move || favorites_count.get().to_string()}
            </span>
        </div>
        <div class="flex-1 overflow-y-auto p-3 min-h-0">
            {move || {
                let effects = favorite_effects.get();
                if effects.is_empty() {
                    view! {
                        <div class="flex flex-col items-center justify-center h-full py-8 text-center">
                            <Icon icon=LuHeart width="24px" height="24px" style="color: var(--fg-tertiary); opacity: 0.3" />
                            <p class="text-xs text-fg-tertiary mt-3">"No favorites yet"</p>
                            <p class="text-[10px] text-fg-tertiary/60 mt-1">"Heart effects to add them here"</p>
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
    }
}

/// Single favorite row — compact, clickable, with unfavorite action.
#[component]
fn FavoriteRow(effect: EffectSummary, delay: String) -> impl IntoView {
    let fx = expect_context::<EffectsContext>();
    let apply_id = effect.id.clone();
    let fav_id = effect.id.clone();
    let active_check_id = effect.id.clone();
    let name = effect.name.clone();
    let category = effect.category.clone();
    let audio_reactive = effect.audio_reactive;
    let (badge_class, _) = category_style(&category);

    let is_active = Signal::derive(move || {
        fx.active_effect_id.get().as_deref() == Some(active_check_id.as_str())
    });

    view! {
        <div
            class="animate-fade-in-up"
            style=delay
        >
            <button
                class=move || {
                    let base = "w-full flex items-center gap-3 px-3 py-2.5 rounded-lg text-left group \
                                transition-all duration-150 btn-press";
                    if is_active.get() {
                        format!("{base} bg-accent-subtle border border-accent-muted")
                    } else {
                        format!("{base} bg-surface-overlay/20 border border-transparent \
                                 hover:bg-surface-hover/40 hover:border-edge-subtle")
                    }
                }
                on:click={
                    let apply_id = apply_id.clone();
                    move |_| fx.apply_effect(apply_id.clone())
                }
            >
                // Active indicator
                {move || is_active.get().then(|| view! {
                    <div class="w-1.5 h-1.5 rounded-full bg-accent animate-pulse shrink-0" />
                })}

                // Name + category
                <div class="flex-1 min-w-0">
                    <div class="text-[12px] text-fg-secondary truncate group-hover:text-fg-primary transition-colors">
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

                // Unfavorite button
                <button
                    class="shrink-0 p-1 rounded opacity-0 group-hover:opacity-100 \
                           hover:bg-surface-hover/60 transition-all duration-150"
                    on:click={
                        let fav_id = fav_id.clone();
                        move |ev: web_sys::MouseEvent| {
                            ev.stop_propagation();
                            fx.toggle_favorite(fav_id.clone());
                        }
                    }
                >
                    <Icon icon=LuX width="12px" height="12px" style="color: var(--fg-tertiary)" />
                </button>
            </button>
        </div>
    }
}

/// Category -> (badge classes, accent hex for gradients).
fn category_style(category: &str) -> (&'static str, &'static str) {
    match category {
        "ambient" => ("bg-neon-cyan/10 text-neon-cyan", "128, 255, 234"),
        "audio" => ("bg-coral/10 text-coral", "255, 106, 193"),
        "gaming" => ("bg-electric-purple/10 text-electric-purple", "225, 53, 255"),
        "reactive" => (
            "bg-electric-yellow/10 text-electric-yellow",
            "241, 250, 140",
        ),
        "generative" => ("bg-success-green/10 text-success-green", "80, 250, 123"),
        "interactive" => ("bg-info-blue/10 text-info-blue", "130, 170, 255"),
        "productivity" => ("bg-pink-soft/10 text-pink-soft", "255, 153, 255"),
        "utility" => ("bg-fg-tertiary/10 text-fg-tertiary", "139, 133, 160"),
        _ => ("bg-surface-overlay/50 text-fg-tertiary", "139, 133, 160"),
    }
}

// ── Performance panel ────────────────────────────────────────────────

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

    // Memo gates: only propagate to DOM when the formatted string actually changes.
    // This prevents re-renders from metrics updates where only unrelated fields moved.
    let engine_text = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("{:.1}/{} fps", m.fps.actual, m.fps.target))
            .unwrap_or_else(|| "—".to_string())
    });
    let engine_hint = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("{} budget misses", m.fps.dropped))
            .unwrap_or_else(|| "render loop".to_string())
    });
    let frame_time_text = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("{:.2} ms avg", m.frame_time.avg_ms))
            .unwrap_or_else(|| "—".to_string())
    });
    let frame_time_hint = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("p95 {:.2} ms", m.frame_time.p95_ms))
            .unwrap_or_else(|| "collecting samples".to_string())
    });
    let preview_text =
        Memo::new(move |_| format!("{:.1}/{} fps", preview_fps.get(), preview_target_fps.get()));
    let preview_hint = Memo::new(move |_| {
        if preview_target_fps.get() <= 15 {
            "debug preview cap".to_string()
        } else {
            "canvas stream delivery".to_string()
        }
    });
    let websocket_text = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format_bytes_per_sec(m.websocket.bytes_sent_per_sec))
            .unwrap_or_else(|| "—".to_string())
    });
    let websocket_hint = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("{} client(s)", m.websocket.client_count))
            .unwrap_or_else(|| "metrics channel".to_string())
    });
    let device_output_text = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| m.devices.output_errors.to_string())
            .unwrap_or_else(|| "0".to_string())
    });
    let device_output_hint = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| {
                format!(
                    "{} devices, {} LEDs",
                    m.devices.connected, m.devices.total_leds
                )
            })
            .unwrap_or_else(|| "device output".to_string())
    });

    let input_stage = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| m.stages.input_sampling_ms)
            .unwrap_or_default()
    });
    let render_stage = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| m.stages.effect_rendering_ms)
            .unwrap_or_default()
    });
    let sample_stage = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| m.stages.spatial_sampling_ms)
            .unwrap_or_default()
    });
    let push_stage = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| m.stages.device_output_ms)
            .unwrap_or_default()
    });
    let publish_stage = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| m.stages.event_bus_ms)
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
                    <p class="text-[11px] text-fg-tertiary">"Engine timing, preview delivery, and websocket transport."</p>
                </div>
                <div class="text-[10px] font-mono text-fg-tertiary rounded-full border border-edge-subtle bg-surface-overlay/30 px-2.5 py-1">
                    {move || {
                        metrics
                            .get()
                            .map(|m| format!("budget misses {}", m.fps.dropped))
                            .unwrap_or_else(|| "metrics warming up".to_string())
                    }}
                </div>
            </div>

            <div class="p-4 space-y-4">
                <div class="grid grid-cols-2 xl:grid-cols-5 gap-3">
                    {metric_card("Engine", engine_text.into(), engine_hint.into())}
                    {metric_card("Preview", preview_text.into(), preview_hint.into())}
                    {metric_card("Frame Time", frame_time_text.into(), frame_time_hint.into())}
                    {metric_card("WebSocket", websocket_text.into(), websocket_hint.into())}
                    // Output errors inline as 5th metric card
                    <div class="rounded-lg border border-edge-subtle bg-surface-overlay/30 px-4 py-3">
                        <div class="text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary mb-1.5">
                            "Output Errors"
                        </div>
                        <div class="text-lg font-medium tabular-nums text-fg-primary">{move || device_output_text.get()}</div>
                        <div class="text-[11px] text-fg-tertiary mt-1">{move || device_output_hint.get()}</div>
                    </div>
                </div>

                <div class="grid grid-cols-2 lg:grid-cols-5 gap-2">
                    {stage_chip("Input", input_stage.into())}
                    {stage_chip("Render", render_stage.into())}
                    {stage_chip("Sample", sample_stage.into())}
                    {stage_chip("Queue", push_stage.into())}
                    {stage_chip("Publish", publish_stage.into())}
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

// ── Status cards ─────────────────────────────────────────────────────

#[component]
fn StatusCards(status: SystemStatus) -> impl IntoView {
    let cards = vec![
        (
            "Status",
            if status.running { "Running" } else { "Stopped" }.to_string(),
            status.running,
        ),
        ("Uptime", format_uptime(status.uptime_seconds), true),
        (
            "Devices",
            status.device_count.to_string(),
            status.device_count > 0,
        ),
        ("Effects", status.effect_count.to_string(), true),
    ];

    view! {
        <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
            {cards.into_iter().enumerate().map(|(i, (label, value, healthy))| {
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

// ── Skeletons & helpers ──────────────────────────────────────────────

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
