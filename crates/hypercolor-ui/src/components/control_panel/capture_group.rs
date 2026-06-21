//! Shared capture pipeline controls — surfaced beside the control panel
//! of screen-reactive effects.
//!
//! These write the daemon's `capture.*` config (live-applied), not effect
//! controls: one screen pipeline feeds every screen-reactive effect and
//! layer, so the knobs are deliberately global and marked as shared. The
//! group renders in the ControlPanel idiom so it reads as part of the
//! effect's rack.

use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_leptos_ext::events::Change;
use hypercolor_types::config::HypercolorConfig;

use crate::api;
use crate::config_state::{ConfigContext, apply_config_key};
use crate::icons::*;

struct CaptureKnob {
    key: &'static str,
    label: &'static str,
    tooltip: &'static str,
    icon: icondata_core::Icon,
    min: f64,
    max: f64,
    step: f64,
    decimals: usize,
    read: fn(&HypercolorConfig) -> f64,
}

fn capture_knobs() -> Vec<CaptureKnob> {
    vec![
        CaptureKnob {
            key: "capture.saturation",
            label: "Saturation",
            tooltip: "Chroma boost on screen zone colors",
            icon: LuPalette,
            min: 0.0,
            max: 2.5,
            step: 0.05,
            decimals: 2,
            read: |cfg| f64::from(cfg.capture.saturation),
        },
        CaptureKnob {
            key: "capture.brightness",
            label: "Brightness",
            tooltip: "Brightness multiplier on screen zone colors",
            icon: LuSun,
            min: 0.0,
            max: 2.0,
            step: 0.05,
            decimals: 2,
            read: |cfg| f64::from(cfg.capture.brightness),
        },
        CaptureKnob {
            key: "capture.gamma",
            label: "Gamma",
            tooltip: "Midtone shaping: above 1.0 deepens darks",
            icon: LuActivity,
            min: 0.4,
            max: 2.5,
            step: 0.05,
            decimals: 2,
            read: |cfg| f64::from(cfg.capture.gamma),
        },
        CaptureKnob {
            key: "capture.smoothing",
            label: "Smoothing",
            tooltip: "Temporal response: low is cinematic, high is twitchy",
            icon: LuTimer,
            min: 0.05,
            max: 1.0,
            step: 0.05,
            decimals: 2,
            read: |cfg| f64::from(cfg.capture.smoothing),
        },
    ]
}

/// Shared capture tuning rack, rendered only for screen-reactive effects.
#[component]
pub fn CaptureSharedControls(
    #[prop(into)] visible: Signal<bool>,
    #[prop(into)] accent_rgb: Signal<String>,
) -> impl IntoView {
    let config_ctx = expect_context::<ConfigContext>();

    let on_change = move |key: String, value: f64| {
        let json_value = serde_json::json!(value);
        config_ctx.set_config.update(|cfg| {
            if let Some(cfg) = cfg {
                apply_config_key(cfg, &key, &json_value);
            }
        });
        leptos::task::spawn_local(async move {
            if let Err(e) = api::set_config_value(&key, &json_value).await {
                leptos::logging::warn!("Capture config set failed: {e}");
                config_ctx.refresh.run(());
            }
        });
    };

    view! {
        <Show when=move || visible.get()>
            <div class="animate-enter-up">
                {move || {
                    let rgb = accent_rgb.get();
                    let line_style = format!(
                        "background: linear-gradient(to right, transparent, rgba({rgb}, 0.3), transparent)"
                    );
                    let label_style = format!("color: rgba({rgb}, 0.78)");
                    view! {
                        <div class="flex items-center gap-2.5 mt-3 mb-1.5 px-1">
                            <div class="h-px flex-1" style=line_style.clone() />
                            <h4
                                class="text-[9px] font-mono uppercase tracking-[0.15em] shrink-0"
                                style=label_style
                                title="Writes the daemon capture pipeline; affects every screen-reactive effect"
                            >
                                "Capture \u{00b7} Shared"
                            </h4>
                            <div class="h-px flex-1" style=line_style />
                        </div>
                    }
                }}
                {capture_knobs()
                    .into_iter()
                    .map(|knob| {
                        let config = config_ctx.config;
                        let value = Signal::derive(move || {
                            config
                                .with(|cfg| cfg.as_ref().map(knob.read))
                                .unwrap_or(1.0)
                        });
                        let fmt_value = move || format!("{:.*}", knob.decimals, value.get());
                        view! {
                            <div
                                class="flex items-center gap-2.5 rounded-lg px-3 py-2 hover:bg-surface-hover/20 transition-colors duration-200 group"
                                title=knob.tooltip
                                style=move || format!("--glow-rgb: {}", accent_rgb.get())
                            >
                                <Icon
                                    icon=knob.icon
                                    width="15px"
                                    height="15px"
                                    style=Signal::derive(move || format!(
                                        "color: rgba({}, 0.55)",
                                        accent_rgb.get()
                                    ))
                                />
                                <label class="text-xs text-fg-secondary font-medium shrink-0 min-w-[80px] max-w-[120px] truncate">
                                    {knob.label}
                                </label>
                                <input
                                    type="range"
                                    class="flex-1 min-w-0 cursor-pointer slider-silk"
                                    min=knob.min
                                    max=knob.max
                                    step=knob.step
                                    prop:value=move || value.get()
                                    on:change=move |ev| {
                                        let event = Change::from_event(ev);
                                        if let Some(v) = event.value::<f64>() {
                                            on_change(knob.key.to_owned(), v);
                                        }
                                    }
                                />
                                <span
                                    class="text-[10px] font-mono tabular-nums w-[36px] text-right shrink-0 px-1.5 py-0.5 rounded"
                                    style=move || {
                                        let rgb = accent_rgb.get();
                                        format!(
                                            "color: rgba({rgb}, 0.9); background: rgba({rgb}, 0.08)"
                                        )
                                    }
                                >
                                    {fmt_value}
                                </span>
                            </div>
                        }
                    })
                    .collect_view()}
            </div>
        </Show>
    }
}
