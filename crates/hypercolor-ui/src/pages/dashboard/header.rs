//! Dashboard header widgets — status strip, preview card, loading skeleton.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::SystemStatus;
use crate::app::{EffectsContext, WsContext};
use crate::color;
use crate::components::canvas_preview::CanvasPreview;
use crate::icons::*;
use crate::style_utils::category_accent_rgb;
use crate::ws::PerformanceMetrics;

// ── Cinematic preview card ────────────────────────────────────────────

/// Cinematic preview with scrim overlay showing active effect info, matching
/// the effects page's treatment. Canvas as background, metadata overlaid at
/// the bottom with category-accent-tinted text.
#[component]
pub(super) fn PreviewCard() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let fx = expect_context::<EffectsContext>();

    let accent_rgb = Signal::derive(move || {
        category_accent_rgb(&fx.active_effect_category.get()).to_string()
    });
    let title_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.86, 0.65));
    let body_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.78, 0.22));
    let meta_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.68, 0.65));

    let effect_meta = Memo::new(move |_| {
        fx.active_effect_id.get().and_then(|id| {
            fx.effects_index.with(|effects| {
                effects
                    .iter()
                    .find(|e| e.effect.id == id)
                    .map(|e| e.effect.clone())
            })
        })
    });

    view! {
        <div
            class="relative rounded-xl overflow-hidden border border-edge-subtle bg-black edge-glow"
            style:--glow-rgb=move || accent_rgb.get()
            style:border-top=move || format!("2px solid rgba({}, 0.45)", accent_rgb.get())
        >
            <CanvasPreview
                frame=ws.canvas_frame
                fps=ws.preview_fps
                show_fps=false
                fps_target=ws.preview_target_fps
                report_presenter_telemetry=true
            />

            // Scrim — transparent at top, fades dark at bottom for legible overlay text
            <div
                class="absolute inset-0 pointer-events-none"
                style="background: linear-gradient(180deg, \
                       rgba(0, 0, 0, 0) 0%, \
                       rgba(0, 0, 0, 0) 40%, \
                       rgba(0, 0, 0, 0.78) 78%, \
                       rgba(0, 0, 0, 0.95) 100%)"
            />

            // Top accent wash — colored highlight along the top edge
            <div
                class="absolute top-0 left-0 right-0 h-px pointer-events-none"
                style=move || format!(
                    "background: linear-gradient(90deg, transparent 0%, rgba({0}, 0.8) 50%, transparent 100%); \
                     box-shadow: 0 0 14px rgba({0}, 0.55)",
                    accent_rgb.get()
                )
            />

            // Info overlay — effect name, description, category + audio badge
            <div class="absolute left-0 right-0 bottom-0 px-3.5 pb-3 pt-8 pointer-events-none">
                {move || {
                    let name = fx.active_effect_name.get();
                    let meta = effect_meta.get();

                    name.map(|effect_name| {
                        let description = meta.as_ref().map(|m| m.description.clone()).unwrap_or_default();
                        let category = meta.as_ref().map(|m| m.category.clone()).unwrap_or_default();
                        let audio_reactive = meta.as_ref().is_some_and(|m| m.audio_reactive);
                        let source = meta.as_ref().map(|m| m.source.clone()).unwrap_or_default();
                        let is_html = source == "html";
                        let show_source = source != "native";

                        view! {
                            <h3
                                class="text-[14px] font-semibold line-clamp-1 leading-tight \
                                       drop-shadow-[0_2px_8px_rgba(0,0,0,0.85)] mb-0.5"
                                style:color=move || format!("rgb({})", title_tint.get())
                            >
                                {effect_name}
                            </h3>

                            {(!description.is_empty()).then(|| view! {
                                <p
                                    class="text-[10px] line-clamp-2 leading-relaxed mb-2 \
                                           drop-shadow-[0_1px_4px_rgba(0,0,0,0.85)]"
                                    style:color=move || format!("rgba({}, 0.88)", body_tint.get())
                                >
                                    {description}
                                </p>
                            })}

                            <div class="flex items-center justify-between gap-2">
                                <div class="flex items-center gap-1.5 min-w-0">
                                    <div
                                        class="w-1.5 h-1.5 rounded-full shrink-0 dot-alive"
                                        style:background=move || format!("rgb({})", accent_rgb.get())
                                        style:box-shadow=move || format!("0 0 6px rgba({}, 0.75)", accent_rgb.get())
                                    />
                                    <span
                                        class="text-[10px] font-mono uppercase tracking-wider capitalize truncate \
                                               drop-shadow-[0_1px_3px_rgba(0,0,0,0.85)]"
                                        style:color=move || format!("rgb({})", meta_tint.get())
                                    >
                                        {category}
                                    </span>
                                </div>
                                <div class="flex items-center gap-1.5 shrink-0">
                                    {show_source.then(|| {
                                        let icon = if is_html { LuGlobe } else { LuCode };
                                        view! {
                                            <span
                                                class="inline-flex items-center text-[9px] font-mono px-1.5 py-0.5 \
                                                       rounded-full bg-white/5 backdrop-blur-sm"
                                                style:color=move || format!("rgba({}, 0.85)", meta_tint.get())
                                            >
                                                <Icon icon=icon width="11px" height="11px" />
                                            </span>
                                        }
                                    })}
                                    {audio_reactive.then(|| view! {
                                        <span
                                            class="inline-flex items-center text-coral/90 px-1.5 py-0.5 \
                                                   rounded-full bg-coral/15 backdrop-blur-sm"
                                            title="Audio-reactive"
                                        >
                                            <Icon icon=LuAudioLines width="11px" height="11px" />
                                        </span>
                                    })}
                                </div>
                            </div>
                        }
                    })
                }}
            </div>
        </div>
    }
}

// ── Status strip ─────────────────────────────────────────────────────

#[component]
pub(super) fn StatusStrip(
    status: SystemStatus,
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
) -> impl IntoView {
    let running = status.running;
    let uptime = format_uptime(status.uptime_seconds);
    let device_count = status.device_count;
    let effect_count = status.effect_count;

    let ws_clients = Memo::new(move |_| {
        metrics.get().map_or(0, |m| m.websocket.client_count)
    });

    view! {
        <div class="px-6 py-3 flex flex-wrap items-center gap-5 animate-fade-in-up border-t border-edge-subtle/10">
            <StatusPill
                label="Status"
                value=if running { "Running" } else { "Stopped" }
                color=if running { "var(--color-success-green)" } else { "var(--color-error-red)" }
                pulsing=running
            />
            <div class="w-px h-6 bg-edge-subtle/30" />
            <StatusPill
                label="Uptime"
                value=uptime.as_str()
                color="var(--color-neon-cyan)"
                pulsing=false
            />
            <div class="w-px h-6 bg-edge-subtle/30" />
            <StatusPill
                label="Devices"
                value=format!("{device_count}")
                color="var(--color-coral)"
                pulsing=false
            />
            <div class="w-px h-6 bg-edge-subtle/30" />
            <StatusPill
                label="Effects"
                value=format!("{effect_count}")
                color="var(--color-electric-purple)"
                pulsing=false
            />
            <div class="w-px h-6 bg-edge-subtle/30" />
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
        <div class="px-6 py-3 border-t border-edge-subtle/10 animate-pulse">
            <div class="flex gap-5">
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
