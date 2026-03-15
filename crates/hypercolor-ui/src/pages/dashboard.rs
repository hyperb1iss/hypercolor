//! Dashboard page — preview + favorites + performance stats.
//!
//! Upgraded from monochrome to full SilkCircuit palette usage with
//! color-coded status cards, health-aware metrics, ambient glow framing,
//! and category-rich favorites.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::{self, EffectSummary, SystemStatus};
use crate::app::{EffectsContext, WsContext};
use crate::components::canvas_preview::CanvasPreview;
use crate::icons::*;
use crate::style_utils::category_style;
use crate::ws::{BackpressureNotice, PerformanceMetrics};

/// Dashboard landing page.
#[component]
pub fn DashboardPage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let status_resource = LocalResource::new(api::fetch_status);

    view! {
        <div class="space-y-5 max-w-6xl animate-fade-in">
            // Top row: preview + favorites side by side
            <div class="grid grid-cols-1 lg:grid-cols-5 gap-5">
                // Live preview — takes 3/5 width, ambient glow frame
                <div class="lg:col-span-3 rounded-xl bg-surface-overlay/60 border border-edge-subtle overflow-hidden preview-glow">
                    <div class="px-4 py-3 border-b border-edge-subtle flex items-center justify-between">
                        <div class="flex items-center gap-2">
                            <Icon icon=LuPlay width="13px" height="13px" style="color: var(--neon-cyan)" />
                            <h2 class="text-[14px] font-medium text-fg-secondary">"Live Preview"</h2>
                        </div>
                        {move || ws.active_effect.get().map(|name| {
                            view! {
                                <div class="flex items-center gap-1.5 bg-accent-subtle rounded-full px-2.5 py-1">
                                    <div class="w-1.5 h-1.5 rounded-full bg-accent animate-pulse" />
                                    <span class="text-[11px] text-accent font-mono">{name}</span>
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

// ── Status cards ─────────────────────────────────────────────────────

#[component]
fn StatusCards(status: SystemStatus) -> impl IntoView {
    let running = status.running;
    let uptime = format_uptime(status.uptime_seconds);
    let device_count = status.device_count;
    let effect_count = status.effect_count;

    view! {
        <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
            // Status — green/red based on running state
            <div class="rounded-xl bg-surface-overlay/60 border border-edge-subtle px-4 py-3.5 animate-fade-in-up stagger-1 group hover:border-edge-default transition-colors duration-200">
                <div class="flex items-center justify-between mb-2.5">
                    <div class="text-[9px] font-mono uppercase tracking-[0.15em] text-fg-tertiary">"Status"</div>
                    <div
                        class="w-7 h-7 rounded-lg flex items-center justify-center"
                        style=move || if running {
                            "background: rgba(80, 250, 123, 0.10)"
                        } else {
                            "background: rgba(255, 99, 99, 0.10)"
                        }
                    >
                        <span style=if running { "color: var(--success-green)" } else { "color: var(--error-red)" }>
                            <Icon icon=LuPower width="14px" height="14px" />
                        </span>
                    </div>
                </div>
                <div class="flex items-center gap-2">
                    <div
                        class="w-2 h-2 rounded-full shrink-0"
                        class=("bg-success-green", running)
                        class=("bg-error-red", !running)
                        class=("animate-pulse", running)
                    />
                    <div
                        class="text-lg font-semibold tabular-nums"
                        class=("text-success-green", running)
                        class=("text-error-red", !running)
                    >
                        {if running { "Running" } else { "Stopped" }}
                    </div>
                </div>
            </div>

            // Uptime — cyan accent
            <div class="rounded-xl bg-surface-overlay/60 border border-edge-subtle px-4 py-3.5 animate-fade-in-up stagger-2 group hover:border-edge-default transition-colors duration-200">
                <div class="flex items-center justify-between mb-2.5">
                    <div class="text-[9px] font-mono uppercase tracking-[0.15em] text-fg-tertiary">"Uptime"</div>
                    <div class="w-7 h-7 rounded-lg flex items-center justify-center" style="background: rgba(128, 255, 234, 0.10)">
                        <Icon icon=LuClock width="14px" height="14px" style="color: var(--neon-cyan)" />
                    </div>
                </div>
                <div class="text-lg font-semibold tabular-nums text-fg-primary">{uptime}</div>
            </div>

            // Devices — coral accent
            <div class="rounded-xl bg-surface-overlay/60 border border-edge-subtle px-4 py-3.5 animate-fade-in-up stagger-3 group hover:border-edge-default transition-colors duration-200">
                <div class="flex items-center justify-between mb-2.5">
                    <div class="text-[9px] font-mono uppercase tracking-[0.15em] text-fg-tertiary">"Devices"</div>
                    <div class="w-7 h-7 rounded-lg flex items-center justify-center" style="background: rgba(255, 106, 193, 0.10)">
                        <Icon icon=LuCpu width="14px" height="14px" style="color: var(--coral)" />
                    </div>
                </div>
                <div class="flex items-baseline gap-1.5">
                    <div
                        class="text-lg font-semibold tabular-nums"
                        class=("text-fg-primary", device_count > 0)
                        class=("text-fg-tertiary", device_count == 0)
                    >
                        {device_count.to_string()}
                    </div>
                    <span class="text-[10px] text-fg-tertiary">"connected"</span>
                </div>
            </div>

            // Effects — purple accent
            <div class="rounded-xl bg-surface-overlay/60 border border-edge-subtle px-4 py-3.5 animate-fade-in-up stagger-4 group hover:border-edge-default transition-colors duration-200">
                <div class="flex items-center justify-between mb-2.5">
                    <div class="text-[9px] font-mono uppercase tracking-[0.15em] text-fg-tertiary">"Effects"</div>
                    <div class="w-7 h-7 rounded-lg flex items-center justify-center" style="background: rgba(225, 53, 255, 0.10)">
                        <Icon icon=LuSparkles width="14px" height="14px" style="color: var(--electric-purple)" />
                    </div>
                </div>
                <div class="flex items-baseline gap-1.5">
                    <div class="text-lg font-semibold tabular-nums text-fg-primary">
                        {effect_count.to_string()}
                    </div>
                    <span class="text-[10px] text-fg-tertiary">"available"</span>
                </div>
            </div>
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
            <span class="text-[10px] font-mono text-coral/80 rounded-full border border-coral/15 bg-coral/[0.06] px-2 py-0.5">
                {move || favorites_count.get().to_string()}
            </span>
        </div>
        <div class="flex-1 overflow-y-auto p-3 min-h-0">
            {move || {
                let effects = favorite_effects.get();
                if effects.is_empty() {
                    view! {
                        <div class="flex flex-col items-center justify-center h-full py-8 text-center">
                            <div class="w-12 h-12 rounded-xl bg-coral/[0.06] border border-coral/10 flex items-center justify-center mb-3">
                                <Icon icon=LuHeart width="20px" height="20px" style="color: var(--coral); opacity: 0.4" />
                            </div>
                            <p class="text-xs text-fg-tertiary">"No favorites yet"</p>
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

/// Single favorite row — compact, clickable, with category accent and unfavorite action.
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

    // Category accent as a left edge bar
    let accent_border = format!("border-left: 2px solid rgba({accent_rgb}, 0.5)");
    let active_accent_border = format!("border-left: 2px solid rgba({accent_rgb}, 0.9)");

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
                    title="Remove from favorites"
                    aria-label="Remove from favorites"
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

// ── Performance panel ────────────────────────────────────────────────

#[component]
fn PerformancePanel(
    #[prop(into)] preview_fps: Signal<f32>,
    #[prop(into)] preview_target_fps: Signal<u32>,
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
    #[prop(into)] backpressure: Signal<Option<BackpressureNotice>>,
) -> impl IntoView {
    // ── Memo gates ──
    let engine_text = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("{:.1}/{} fps", m.fps.actual, m.fps.target))
            .unwrap_or_else(|| "\u{2014}".to_string())
    });
    let engine_hint = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("{} budget misses", m.fps.dropped))
            .unwrap_or_else(|| "render loop".to_string())
    });
    #[allow(clippy::cast_precision_loss)]
    let engine_health = Memo::new(move |_| {
        metrics.get().map_or(Health::Neutral, |m| {
            let ratio = if m.fps.target > 0 {
                m.fps.actual / (m.fps.target as f64)
            } else {
                1.0
            };
            if ratio >= 0.9 {
                Health::Good
            } else if ratio >= 0.7 {
                Health::Warn
            } else {
                Health::Bad
            }
        })
    });

    let frame_time_text = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("{:.2} ms", m.frame_time.avg_ms))
            .unwrap_or_else(|| "\u{2014}".to_string())
    });
    let frame_time_hint = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format!("p95 {:.2} ms", m.frame_time.p95_ms))
            .unwrap_or_else(|| "collecting samples".to_string())
    });
    #[allow(clippy::cast_precision_loss)]
    let frame_time_health = Memo::new(move |_| {
        metrics.get().map_or(Health::Neutral, |m| {
            let budget = if m.fps.target > 0 {
                1000.0 / (m.fps.target as f64)
            } else {
                33.33
            };
            if m.frame_time.avg_ms <= budget * 0.8 {
                Health::Good
            } else if m.frame_time.avg_ms <= budget {
                Health::Warn
            } else {
                Health::Bad
            }
        })
    });

    let preview_text =
        Memo::new(move |_| format!("{:.1}/{} fps", preview_fps.get(), preview_target_fps.get()));
    let preview_hint = Memo::new(move |_| {
        if preview_target_fps.get() <= 15 {
            "debug preview cap".to_string()
        } else {
            "canvas stream".to_string()
        }
    });
    #[allow(clippy::cast_precision_loss)]
    let preview_health = Memo::new(move |_| {
        let target = preview_target_fps.get();
        if target == 0 {
            return Health::Neutral;
        }
        let ratio = preview_fps.get() / (target as f32);
        if ratio >= 0.85 {
            Health::Good
        } else if ratio >= 0.6 {
            Health::Warn
        } else {
            Health::Bad
        }
    });

    let websocket_text = Memo::new(move |_| {
        metrics
            .get()
            .map(|m| format_bytes_per_sec(m.websocket.bytes_sent_per_sec))
            .unwrap_or_else(|| "\u{2014}".to_string())
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
    let output_health = Memo::new(move |_| {
        metrics.get().map_or(Health::Neutral, |m| {
            if m.devices.output_errors == 0 {
                Health::Good
            } else if m.devices.output_errors < 10 {
                Health::Warn
            } else {
                Health::Bad
            }
        })
    });

    // ── Pipeline stage memos ──
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

    view! {
        <div class="rounded-xl bg-surface-overlay/60 border border-edge-subtle overflow-hidden">
            <div class="px-4 py-3 border-b border-edge-subtle flex items-center justify-between gap-3">
                <div class="flex items-center gap-2.5">
                    <Icon icon=LuActivity width="15px" height="15px" style="color: var(--neon-cyan)" />
                    <div>
                        <h2 class="text-[14px] font-medium text-fg-secondary">"Performance"</h2>
                        <p class="text-[11px] text-fg-tertiary">"Engine timing, preview delivery, and device output."</p>
                    </div>
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
                // Metric cards row — color-coded by health
                <div class="grid grid-cols-2 xl:grid-cols-5 gap-3">
                    <MetricCard
                        label="Engine"
                        icon=LuGauge
                        value=Signal::derive(move || engine_text.get())
                        hint=Signal::derive(move || engine_hint.get())
                        health=Signal::derive(move || engine_health.get())
                    />
                    <MetricCard
                        label="Preview"
                        icon=LuMonitor
                        value=Signal::derive(move || preview_text.get())
                        hint=Signal::derive(move || preview_hint.get())
                        health=Signal::derive(move || preview_health.get())
                    />
                    <MetricCard
                        label="Frame Time"
                        icon=LuTimer
                        value=Signal::derive(move || frame_time_text.get())
                        hint=Signal::derive(move || frame_time_hint.get())
                        health=Signal::derive(move || frame_time_health.get())
                    />
                    <MetricCard
                        label="WebSocket"
                        icon=LuWifi
                        value=Signal::derive(move || websocket_text.get())
                        hint=Signal::derive(move || websocket_hint.get())
                        health=Signal::derive(|| Health::Neutral)
                    />
                    <MetricCard
                        label="Output Errors"
                        icon=LuTriangleAlert
                        value=Signal::derive(move || device_output_text.get())
                        hint=Signal::derive(move || device_output_hint.get())
                        health=Signal::derive(move || output_health.get())
                    />
                </div>

                // Pipeline stages — with color accents per stage
                <div>
                    <div class="text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary mb-2 px-1">"Pipeline Stages"</div>
                    <div class="grid grid-cols-2 lg:grid-cols-5 gap-2">
                        <StageChip label="Input"   value=Signal::derive(move || input_stage.get())   color="var(--neon-cyan)" />
                        <StageChip label="Render"  value=Signal::derive(move || render_stage.get())  color="var(--electric-purple)" />
                        <StageChip label="Sample"  value=Signal::derive(move || sample_stage.get())  color="var(--coral)" />
                        <StageChip label="Queue"   value=Signal::derive(move || push_stage.get())    color="var(--electric-yellow)" />
                        <StageChip label="Publish" value=Signal::derive(move || publish_stage.get()) color="var(--success-green)" />
                    </div>
                </div>

                {move || backpressure.get().map(|notice| {
                    view! {
                        <div class="rounded-lg border border-electric-yellow/20 bg-electric-yellow/[0.06] px-3 py-2 text-[12px] text-electric-yellow flex items-center gap-2">
                            <Icon icon=LuTriangleAlert width="14px" height="14px" style="color: var(--electric-yellow); flex-shrink: 0" />
                            <div>
                                <span class="font-mono uppercase tracking-[0.14em] mr-2">"Backpressure"</span>
                                {format!(
                                    "{} dropped on {}. {} \u{2192} {} fps",
                                    notice.dropped_frames,
                                    notice.channel,
                                    notice.recommendation.replace('_', " "),
                                    notice.suggested_fps
                                )}
                            </div>
                        </div>
                    }
                })}
            </div>
        </div>
    }
}

// ── Health-aware metric card ─────────────────────────────────────────

/// Visual health state for metric cards.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Health {
    Good,
    Warn,
    Bad,
    Neutral,
}

impl Health {
    fn border_class(self) -> &'static str {
        match self {
            Self::Good => "border-success-green/20",
            Self::Warn => "border-electric-yellow/20",
            Self::Bad => "border-error-red/20",
            Self::Neutral => "border-edge-subtle",
        }
    }

    fn icon_color(self) -> &'static str {
        match self {
            Self::Good => "color: var(--success-green)",
            Self::Warn => "color: var(--electric-yellow)",
            Self::Bad => "color: var(--error-red)",
            Self::Neutral => "color: var(--fg-tertiary)",
        }
    }

    fn value_class(self) -> &'static str {
        match self {
            Self::Good => "text-fg-primary",
            Self::Warn => "text-electric-yellow",
            Self::Bad => "text-error-red",
            Self::Neutral => "text-fg-primary",
        }
    }

    fn dot_bg(self) -> &'static str {
        match self {
            Self::Good => "background: var(--success-green)",
            Self::Warn => "background: var(--electric-yellow)",
            Self::Bad => "background: var(--error-red)",
            Self::Neutral => "background: var(--fg-tertiary)",
        }
    }
}

/// A single performance metric card with icon, health coloring, and status dot.
#[component]
fn MetricCard(
    label: &'static str,
    icon: icondata::Icon,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] hint: Signal<String>,
    #[prop(into)] health: Signal<Health>,
) -> impl IntoView {
    // Icon style prop is MaybeProp<String> — cannot take closures.
    // Use inline style on a wrapper span instead for reactive coloring.
    view! {
        <div class=move || format!(
            "rounded-lg border bg-surface-overlay/30 px-4 py-3 transition-colors duration-300 {}",
            health.get().border_class()
        )>
            <div class="flex items-center justify-between mb-1.5">
                <div class="text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary">
                    {label}
                </div>
                <div class="flex items-center gap-1.5">
                    <div
                        class="w-1.5 h-1.5 rounded-full transition-colors duration-300"
                        style=move || health.get().dot_bg()
                    />
                    <span style=move || health.get().icon_color()>
                        <Icon icon=icon width="12px" height="12px" />
                    </span>
                </div>
            </div>
            <div class=move || format!(
                "text-lg font-semibold tabular-nums transition-colors duration-300 {}",
                health.get().value_class()
            )>
                {move || value.get()}
            </div>
            <div class="text-[11px] text-fg-tertiary mt-1">{move || hint.get()}</div>
        </div>
    }
}

// ── Pipeline stage chip ──────────────────────────────────────────────

/// A compact pipeline stage chip with a colored left accent.
#[component]
fn StageChip(
    label: &'static str,
    #[prop(into)] value: Signal<f64>,
    color: &'static str,
) -> impl IntoView {
    let border_style = format!("border-left: 2px solid {color}");

    view! {
        <div
            class="rounded-lg border border-edge-subtle bg-surface-overlay/30 px-3 py-2"
            style=border_style
        >
            <div class="text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary mb-1">
                {label}
            </div>
            <div class="text-sm tabular-nums text-fg-secondary">
                {move || format!("{:.2} ms", value.get())}
            </div>
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
