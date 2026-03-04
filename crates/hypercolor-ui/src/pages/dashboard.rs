//! Dashboard page — system overview with mini preview and quick-switch.

use leptos::prelude::*;

use crate::api::{self, EffectSummary, SystemStatus};
use crate::components::canvas_preview::CanvasPreview;
use crate::ws::WsManager;

/// Dashboard landing page.
#[component]
pub fn DashboardPage() -> impl IntoView {
    let status_resource = Resource::new(|| (), |_| api::fetch_status());
    let effects_resource = Resource::new(|| (), |_| api::fetch_effects());

    // WebSocket for mini preview
    let ws = StoredValue::new(WsManager::new());
    let canvas_frame = Signal::derive(move || ws.with_value(|w| w.canvas_frame.get()));
    let ws_fps = Signal::derive(move || ws.with_value(|w| w.fps.get()));

    view! {
        <div class="space-y-8 max-w-5xl">
            // Hero section
            <div>
                <h1 class="text-xl font-medium text-zinc-100 mb-1">"Dashboard"</h1>
                <p class="text-sm text-zinc-500">"Hypercolor lighting engine overview"</p>
            </div>

            // Status cards row
            <Suspense fallback=move || view! { <StatusSkeleton /> }>
                {move || status_resource.get().map(|result| {
                    match result {
                        Ok(status) => view! { <StatusCards status=status /> }.into_any(),
                        Err(e) => view! {
                            <div class="text-sm text-error-red bg-error-red/5 border border-error-red/20 rounded-lg px-4 py-3">
                                "Failed to connect: " {e}
                            </div>
                        }.into_any(),
                    }
                })}
            </Suspense>

            // Main content — preview + quick switch
            <div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
                // Live preview
                <div class="rounded-xl bg-layer-2 border border-white/5 overflow-hidden">
                    <div class="px-4 py-3 border-b border-white/5">
                        <h2 class="text-sm font-medium text-zinc-300">"Live Preview"</h2>
                    </div>
                    <div class="p-4">
                        <CanvasPreview
                            frame=canvas_frame
                            fps=ws_fps
                            show_fps=true
                        />
                    </div>
                </div>

                // Quick switch
                <div class="rounded-xl bg-layer-2 border border-white/5">
                    <div class="px-4 py-3 border-b border-white/5">
                        <h2 class="text-sm font-medium text-zinc-300">"Quick Switch"</h2>
                    </div>
                    <div class="p-4">
                        <Suspense fallback=move || view! {
                            <div class="text-xs text-zinc-600">"Loading effects..."</div>
                        }>
                            {move || effects_resource.get().map(|result| {
                                match result {
                                    Ok(effects) => {
                                        let runnable: Vec<_> = effects.into_iter()
                                            .filter(|e| e.runnable)
                                            .take(12)
                                            .collect();
                                        view! { <QuickSwitchGrid effects=runnable /> }.into_any()
                                    }
                                    Err(_) => view! {
                                        <p class="text-xs text-zinc-600">"Could not load effects"</p>
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
        ("Status", if status.running { "Running" } else { "Stopped" }, status.running),
        ("Uptime", &format_uptime(status.uptime_seconds), true),
        ("Devices", &status.device_count.to_string(), status.device_count > 0),
        ("Effects", &status.effect_count.to_string(), true),
    ];

    view! {
        <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
            {cards.into_iter().map(|(label, value, healthy)| {
                let value = value.to_string();
                view! {
                    <div class="rounded-xl bg-layer-2 border border-white/5 px-4 py-3">
                        <div class="text-[10px] font-mono uppercase tracking-widest text-zinc-600 mb-1">{label}</div>
                        <div class="text-lg font-medium tabular-nums" class=("text-zinc-100", healthy) class=("text-zinc-500", !healthy)>
                            {value}
                        </div>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

/// Quick-switch effect grid — compact one-click apply buttons.
#[component]
fn QuickSwitchGrid(effects: Vec<EffectSummary>) -> impl IntoView {
    if effects.is_empty() {
        return view! {
            <p class="text-xs text-zinc-600 py-4 text-center">"No runnable effects found"</p>
        }
        .into_any();
    }

    view! {
        <div class="grid grid-cols-2 gap-1.5">
            {effects.into_iter().map(|effect| {
                let id = effect.id.clone();
                let name = effect.name.clone();
                let category = effect.category.clone();
                view! {
                    <button
                        class="text-left px-3 py-2 rounded-lg bg-white/[0.02] border border-white/5
                               hover:bg-white/[0.05] hover:border-white/10 transition-all duration-150 group"
                        on:click=move |_| {
                            let id = id.clone();
                            leptos::task::spawn_local(async move {
                                let _ = api::apply_effect(&id).await;
                            });
                        }
                    >
                        <div class="text-xs text-zinc-300 truncate group-hover:text-zinc-100 transition-colors">{name}</div>
                        <div class="text-[10px] text-zinc-600">{category}</div>
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
                <div class="rounded-xl bg-layer-2 border border-white/5 px-4 py-3 animate-pulse">
                    <div class="h-3 w-12 bg-white/5 rounded mb-2" />
                    <div class="h-6 w-16 bg-white/5 rounded" />
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
