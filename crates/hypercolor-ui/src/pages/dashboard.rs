//! Dashboard page — system overview with mini preview and quick-switch.

use leptos::prelude::*;

use crate::api::{self, EffectSummary, SystemStatus};
use crate::app::WsContext;
use crate::components::canvas_preview::CanvasPreview;

/// Dashboard landing page.
#[component]
pub fn DashboardPage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let status_resource = LocalResource::new(api::fetch_status);
    let effects_resource = LocalResource::new(api::fetch_effects);

    let canvas_frame = Signal::derive(move || ws.canvas_frame.get());
    let ws_fps = Signal::derive(move || ws.fps.get());

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
                        Ok(status) => view! { <StatusCards status=status /> }.into_any(),
                        Err(e) => view! {
                            <div class="text-sm text-status-error bg-status-error/[0.05] border border-status-error/10 rounded-lg px-4 py-3">
                                "Failed to connect: " {e}
                            </div>
                        }.into_any(),
                    }
                })}
            </Suspense>

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
                            fps=ws_fps
                            show_fps=true
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
                            {move || effects_resource.get().map(|result| {
                                match result {
                                    Ok(effects) => {
                                        let runnable: Vec<_> = effects.into_iter()
                                            .filter(|e| e.runnable)
                                            .take(20)
                                            .collect();
                                        view! { <QuickSwitchGrid effects=runnable /> }.into_any()
                                    }
                                    Err(_) => view! {
                                        <p class="text-xs text-fg-tertiary">"Could not load effects"</p>
                                    }.into_any(),
                                }
                            })}
                        </Suspense>
                    </div>
                </div>
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
fn QuickSwitchGrid(effects: Vec<EffectSummary>) -> impl IntoView {
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
                        on:click=move |_| {
                            let id = id.clone();
                            leptos::task::spawn_local(async move {
                                let _ = api::apply_effect(&id).await;
                            });
                        }
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
