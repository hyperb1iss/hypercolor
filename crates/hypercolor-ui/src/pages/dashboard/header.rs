//! Dashboard header widgets — status strip and loading skeleton.
//!
//! The preview widget lives in the shared `components::preview_cabinet`
//! module so both the dashboard and the effects page render the same
//! cinematic cabinet.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::SystemStatus;
use crate::components::scene_switcher::{
    SceneSwitcherMenu, active_scene_label, active_scene_locked,
};
use crate::components::status_pill::StatusPill;
use crate::icons::*;
use crate::ws::PerformanceMetrics;
use crate::zones::ScenesContext;

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

    let ws_clients =
        Memo::new(move |_| metrics.with(|m| m.as_ref().map_or(0, |m| m.websocket.client_count)));

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
            <ScenePill
                fallback_scene=active_scene
                fallback_locked=active_scene_snapshot_locked
            />
            <div class="w-px h-5 bg-edge-subtle/30" />
            <StatusPillDynamic
                label="WS Clients"
                value=Memo::new(move |_| ws_clients.get().to_string())
                color="var(--color-electric-yellow)"
            />
        </div>
    }
}

/// The status strip's scene pill. The label comes from the shared scene
/// resource (so external switches stay fresh), falling back to the
/// page's one-shot status snapshot while that resource loads. With more
/// than one scene to pick from, the pill becomes a switcher trigger;
/// otherwise it stays the familiar static pill. Lock styling is kept
/// for snapshot-locked scenes.
#[component]
fn ScenePill(fallback_scene: Option<String>, fallback_locked: bool) -> impl IntoView {
    let scenes_ctx = expect_context::<ScenesContext>();
    let (open, set_open) = signal(false);

    // `(value, locked, interactive)` for the pill, or `None` to render
    // nothing (default scene with nowhere to switch to).
    let pill = Memo::new(move |_| {
        let interactive = scenes_ctx.has_multiple();
        scenes_ctx.active.with(|active| {
            let is_saved = active
                .as_ref()
                .is_some_and(|scene| scene.kind != hypercolor_types::scene::SceneKind::Ephemeral);
            if is_saved {
                return Some((
                    active_scene_label(active.as_ref()),
                    active_scene_locked(active.as_ref()),
                    interactive,
                ));
            }
            if active.is_none()
                && let Some(name) = fallback_scene.clone()
            {
                return Some((name, fallback_locked, interactive));
            }
            interactive.then(|| ("Default".to_owned(), false, true))
        })
    });

    view! {
        {move || pill.get().map(|(value, locked, interactive)| {
            let label = if locked { "Scene Lock" } else { "Scene" };
            let color = if locked {
                "var(--color-electric-yellow)"
            } else {
                "var(--color-neon-cyan)"
            };
            let value_text = if locked { format!("{value} · snap") } else { value };
            view! {
                <div class="w-px h-5 bg-edge-subtle/30" />
                {if interactive {
                    view! {
                        <div class="relative dashboard-scene-pill">
                            <button
                                type="button"
                                class="group flex items-center gap-1.5 rounded-lg px-1.5 py-1 \
                                       -mx-1.5 -my-1 transition-colors hover:bg-surface-hover/30 \
                                       focus-visible:outline-none focus-visible:ring-1 \
                                       focus-visible:ring-accent/50 btn-press"
                                title="Switch scene"
                                aria-haspopup="menu"
                                aria-expanded=move || open.get().to_string()
                                on:click=move |_| set_open.update(|value| *value = !*value)
                            >
                                <StatusPill
                                    label=label
                                    value=value_text
                                    color=color
                                    pulsing=false
                                />
                                <span class="text-fg-tertiary group-hover:text-fg-secondary transition-colors">
                                    <Icon icon=LuChevronDown width="12px" height="12px" />
                                </span>
                            </button>
                            <SceneSwitcherMenu
                                anchor_class="dashboard-scene-pill"
                                is_open=open
                                set_open=set_open
                                placement="left-0 top-full mt-2 w-52"
                            />
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <StatusPill label=label value=value_text color=color pulsing=false />
                    }.into_any()
                }}
            }
        })}
    }
}

#[component]
fn StatusPillDynamic(
    label: &'static str,
    #[prop(into)] value: Signal<String>,
    color: &'static str,
) -> impl IntoView {
    view! {
        {move || view! { <StatusPill label=label value=value.get() color=color pulsing=false /> }}
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
