//! Dashboard header widgets — status strip and loading skeleton.
//!
//! The preview widget lives in the shared `components::preview_cabinet`
//! module so both the dashboard and the effects page render the same
//! cinematic cabinet.

use leptos::prelude::*;

use crate::api::SystemStatus;
use crate::components::status_pill::StatusPill;
use crate::live_uptime::{format_uptime, use_live_uptime};

// ── Status strip ─────────────────────────────────────────────────────

/// Inline status pills — no outer padding or border, so it can sit on the
/// same row as the page title in the dashboard header.
#[component]
pub(super) fn StatusStrip(status: SystemStatus) -> impl IntoView {
    let running = status.running;
    let live_uptime = use_live_uptime(status.uptime_seconds);
    let uptime = Signal::derive(move || format_uptime(live_uptime.get()));
    let device_count = status.device_count;
    let effect_count = status.effect_count;

    let status_color = if running {
        "var(--color-status-success)"
    } else {
        "var(--color-status-error)"
    };

    view! {
        // Desktop keeps the pill grammar, minus the WS Clients pill —
        // that number is developer telemetry and lives with the perf
        // panels. Phones condense to one mono line where the pulsing dot
        // carries the running state.
        <div class="flex items-center gap-4 shrink-0 max-md:gap-2.5">
            <div class="max-md:hidden flex items-center gap-4">
                <StatusPill
                    label="Status"
                    value=if running { "Running" } else { "Stopped" }
                    color=status_color
                    pulsing=running
                />
                <div class="w-px h-5 bg-edge-subtle/30" />
                <StatusPill
                    label="Uptime"
                    value=uptime
                    color="var(--color-cyan)"
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
                    color="var(--color-accent)"
                    pulsing=false
                />
            </div>
            <div class="md:hidden flex items-center gap-2 min-w-0 text-[11px] font-mono text-fg-tertiary">
                <div
                    class="w-2 h-2 rounded-full shrink-0"
                    class=("animate-pulse", running)
                    style=format!("background: {status_color}; box-shadow: 0 0 8px {status_color}")
                />
                <span class="tabular-nums shrink-0">{move || uptime.get()}</span>
                <span class="text-fg-tertiary/40">"·"</span>
                <span class="tabular-nums shrink-0">{format!("{device_count} dev")}</span>
                <span class="text-fg-tertiary/40">"·"</span>
                <span class="tabular-nums shrink-0">{format!("{effect_count} fx")}</span>
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
