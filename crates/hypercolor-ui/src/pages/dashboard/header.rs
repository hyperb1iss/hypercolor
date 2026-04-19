//! Dashboard header widgets — status strip and loading skeleton.
//!
//! The preview widget lives in the shared `components::preview_cabinet`
//! module so both the dashboard and the effects page render the same
//! cinematic cabinet.

use leptos::prelude::*;

use crate::api::SystemStatus;
use crate::ws::PerformanceMetrics;

// ── Status strip ─────────────────────────────────────────────────────

/// Inline status pills — no outer padding or border, so it can sit on the
/// same row as the page title in the dashboard header.
#[component]
pub(super) fn StatusStrip(
    status: SystemStatus,
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
) -> impl IntoView {
    let running = status.running;
    let uptime = format_uptime(status.uptime_seconds);
    let device_count = status.device_count;
    let effect_count = status.effect_count;
    let active_scene = status.active_scene;
    let active_scene_snapshot_locked = status.active_scene_snapshot_locked;

    let ws_clients = Memo::new(move |_| metrics.get().map_or(0, |m| m.websocket.client_count));

    view! {
        <div class="flex items-center gap-4 shrink-0">
            <StatusPill
                label="Status"
                value=if running { "Running" } else { "Stopped" }
                color=if running { "var(--color-success-green)" } else { "var(--color-error-red)" }
                pulsing=running
            />
            <div class="w-px h-5 bg-edge-subtle/30" />
            <StatusPill
                label="Uptime"
                value=uptime.as_str()
                color="var(--color-neon-cyan)"
                pulsing=false
            />
            <div class="w-px h-5 bg-edge-subtle/30" />
            <StatusPill
                label="Devices"
                value=format!("{device_count}")
                color="var(--color-coral)"
                pulsing=false
            />
            <div class="w-px h-5 bg-edge-subtle/30" />
            <StatusPill
                label="Effects"
                value=format!("{effect_count}")
                color="var(--color-electric-purple)"
                pulsing=false
            />
            {active_scene.as_ref().map(|scene| view! {
                <div class="w-px h-5 bg-edge-subtle/30" />
                <StatusPill
                    label=if active_scene_snapshot_locked { "Scene Lock" } else { "Scene" }
                    value=if active_scene_snapshot_locked {
                        format!("{scene} · snap")
                    } else {
                        scene.clone()
                    }
                    color=if active_scene_snapshot_locked {
                        "var(--color-electric-yellow)"
                    } else {
                        "var(--color-neon-cyan)"
                    }
                    pulsing=false
                />
            })}
            <div class="w-px h-5 bg-edge-subtle/30" />
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

// ── Skeleton ─────────────────────────────────────────────────────────

#[component]
pub(super) fn StatusSkeleton() -> impl IntoView {
    view! {
        <div class="flex items-center gap-4 shrink-0 animate-pulse">
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
