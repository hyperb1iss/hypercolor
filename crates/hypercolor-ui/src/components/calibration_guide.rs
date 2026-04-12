//! Calibration guide — effect-specific setup workflow help for layout tuning.

use std::collections::HashMap;

use hypercolor_types::effect::{ControlValue, PresetTemplate};
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::app::EffectsContext;
use crate::components::preset_matching::bundled_preset_to_json;
use crate::icons::*;
use crate::toasts;

#[derive(Clone, Copy)]
struct WorkflowStep {
    preset: &'static str,
    title: &'static str,
    summary: &'static str,
    icon: icondata::Icon,
}

#[derive(Clone, Copy)]
struct DiagnosisTip {
    title: &'static str,
    body: &'static str,
    icon: icondata::Icon,
}

const WORKFLOW_STEPS: [WorkflowStep; 6] = [
    WorkflowStep {
        preset: "Horizontal Sweep",
        title: "Place Left / Right",
        summary: "Run a slow horizontal pass first to find each device on the canvas and catch reversed strip order.",
        icon: LuMonitor,
    },
    WorkflowStep {
        preset: "Vertical Sweep",
        title: "Place Top / Bottom",
        summary: "Confirm stacked or vertical devices sit where you expect before touching rotation.",
        icon: LuMonitor,
    },
    WorkflowStep {
        preset: "Corner Compass",
        title: "Fix Rotation",
        summary: "Cycle the corners to spot zones that are rotated or mirrored against the rest of the layout.",
        icon: LuRotateCcw,
    },
    WorkflowStep {
        preset: "Quadrant Clock",
        title: "Confirm Orientation",
        summary: "Use the quadrant order to verify the whole layout reads clockwise the way your eye expects.",
        icon: LuGrid2x2,
    },
    WorkflowStep {
        preset: "Diagonal Crosshair",
        title: "Fine Tune Position",
        summary: "Walk an intersection diagonally through the rig to tighten placement and find footprints that are offset.",
        icon: LuRadar,
    },
    WorkflowStep {
        preset: "Expanding Rings",
        title: "Check Footprint",
        summary: "Use rings last to validate centering, scaling, and whether any zone is clipped or oversized.",
        icon: LuCircleDot,
    },
];

const DIAGNOSIS_TIPS: [DiagnosisTip; 4] = [
    DiagnosisTip {
        title: "Sweep runs the wrong way",
        body: "The zone is likely mirrored, its strip order is reversed, or it needs a 180° rotation.",
        icon: LuUndo2,
    },
    DiagnosisTip {
        title: "Corner colors land in the wrong corners",
        body: "Rotation is off. If the order also flips, you are probably mirrored, not just rotated.",
        icon: LuRotateCcw,
    },
    DiagnosisTip {
        title: "Crosshair misses part of a device",
        body: "The zone position or size is wrong. Move it first, then resize only if the footprint still feels clipped.",
        icon: LuRadar,
    },
    DiagnosisTip {
        title: "Rings look off-center or chopped",
        body: "The zone anchor is displaced or the footprint is too small for the physical device bounds.",
        icon: LuCircle,
    },
];

/// Calibration-specific guide panel shown on the effects page.
#[component]
pub fn CalibrationGuide(
    #[prop(into)] effect_id: Signal<Option<String>>,
    #[prop(into)] control_values: Signal<HashMap<String, ControlValue>>,
    #[prop(into)] accent_rgb: Signal<String>,
) -> impl IntoView {
    let fx = expect_context::<EffectsContext>();

    let bundled_presets = LocalResource::new(move || {
        let id = effect_id.get();
        async move {
            if let Some(id) = id {
                api::fetch_bundled_presets(&id).await.unwrap_or_default()
            } else {
                Vec::<PresetTemplate>::new()
            }
        }
    });

    let preset_map = Memo::new(move |_| {
        bundled_presets
            .get()
            .unwrap_or_default()
            .into_iter()
            .map(|preset| (preset.name.clone(), preset))
            .collect::<HashMap<_, _>>()
    });

    let current_pattern = Memo::new(move |_| {
        control_label(&control_values.get(), "pattern").unwrap_or_else(|| "Sweep".to_owned())
    });
    let current_direction = Memo::new(move |_| {
        control_label(&control_values.get(), "direction")
            .unwrap_or_else(|| "Left to Right".to_owned())
    });
    let grid_enabled = Memo::new(move |_| control_bool(&control_values.get(), "show_grid"));
    let current_speed =
        Memo::new(move |_| control_number(&control_values.get(), "speed").unwrap_or(18.0));
    let current_size =
        Memo::new(move |_| control_number(&control_values.get(), "size").unwrap_or(22.0));

    let current_focus = Memo::new(move |_| {
        calibration_focus(
            current_pattern.get().as_str(),
            current_direction.get().as_str(),
            grid_enabled.get(),
            current_speed.get(),
            current_size.get(),
        )
    });

    let next_recommended =
        Memo::new(move |_| next_step_for_pattern(current_pattern.get().as_str()));

    view! {
        <div
            class="rounded-xl border border-edge-subtle bg-surface-raised/80 p-3 edge-glow animate-fade-in-up"
            style:border-top=move || format!("2px solid rgba({}, 0.24)", accent_rgb.get())
        >
            <div class="flex items-start gap-2.5 mb-3">
                <div class="w-8 h-8 rounded-lg bg-neon-cyan/10 text-neon-cyan flex items-center justify-center shrink-0">
                    <Icon icon=LuRadar width="15px" height="15px" />
                </div>
                <div class="min-w-0">
                    <div class="text-[11px] font-semibold uppercase tracking-[0.16em] text-fg-secondary">
                        "Calibration Guide"
                    </div>
                    <p class="text-[12px] leading-relaxed text-fg-tertiary mt-1">
                        "Use the presets below in order so placement, rotation, and footprint problems become obvious before you start fine tuning."
                    </p>
                </div>
            </div>

            <div class="rounded-lg border border-neon-cyan/15 bg-black/20 px-3 py-2.5 mb-3">
                <div class="flex items-center gap-1.5 text-[10px] font-mono uppercase tracking-[0.14em] text-neon-cyan/80 mb-1.5">
                    <Icon icon=LuCircleDot width="11px" height="11px" />
                    "Current Pass"
                </div>
                <div class="flex flex-wrap gap-1.5 mb-2">
                    <GuideChip label=current_pattern />
                    <GuideChip label=current_direction />
                    <GuideChip label=Signal::derive(move || {
                        if grid_enabled.get() {
                            "Grid On".to_string()
                        } else {
                            "Grid Off".to_string()
                        }
                    }) />
                </div>
                <p class="text-[12px] leading-relaxed text-fg-secondary">
                    {move || current_focus.get()}
                </p>
                <p class="text-[11px] leading-relaxed text-fg-tertiary mt-2">
                    {move || format!("Next recommended pass: {}", next_recommended.get())}
                </p>
            </div>

            <div class="mb-3">
                <div class="flex items-center gap-1.5 text-[10px] font-mono uppercase tracking-[0.14em] text-fg-tertiary/75 mb-2">
                    <Icon icon=LuZap width="11px" height="11px" />
                    "Recommended Sequence"
                </div>
                <div class="space-y-2">
                    {WORKFLOW_STEPS.into_iter().enumerate().map(|(index, step)| {
                        let template = Signal::derive({
                            let name = step.preset.to_owned();
                            move || preset_map.get().get(&name).cloned()
                        });
                        let description = Signal::derive(move || {
                            template
                                .get()
                                .and_then(|preset| preset.description)
                                .unwrap_or_else(|| step.summary.to_owned())
                        });
                        let on_apply = move |_| {
                            let Some(template) = template.get_untracked() else {
                                toasts::toast_error("Calibration preset is unavailable");
                                return;
                            };
                            let controls_json = bundled_preset_to_json(&template.controls);
                            let preset_name = template.name.clone();
                            leptos::task::spawn_local(async move {
                                match api::update_controls(&controls_json).await {
                                    Ok(()) => {
                                        fx.refresh_active_effect();
                                        toasts::toast_success(&format!("Loaded {preset_name}"));
                                    }
                                    Err(error) => {
                                        toasts::toast_error(&format!(
                                            "Failed to load calibration preset: {error}"
                                        ));
                                    }
                                }
                            });
                        };
                        view! {
                            <div class="rounded-lg border border-edge-subtle/70 bg-black/15 px-3 py-2.5">
                                <div class="flex items-start gap-2.5">
                                    <div class="w-6 h-6 rounded-md bg-white/5 border border-white/6 flex items-center justify-center shrink-0 mt-0.5">
                                        <span class="text-[10px] font-mono text-fg-tertiary/75">{index + 1}</span>
                                    </div>
                                    <div class="min-w-0 flex-1">
                                        <div class="flex items-center gap-2 mb-1">
                                            <span class="text-fg-secondary/80">
                                                <Icon icon=step.icon width="12px" height="12px" />
                                            </span>
                                            <div class="text-[12px] font-semibold text-fg-secondary">{step.title}</div>
                                        </div>
                                        <p class="text-[11px] leading-relaxed text-fg-tertiary mb-2">
                                            {move || description.get()}
                                        </p>
                                        <div class="flex items-center justify-between gap-2">
                                            <div class="text-[10px] font-mono uppercase tracking-[0.12em] text-fg-tertiary/60 truncate">
                                                {step.preset}
                                            </div>
                                            <button
                                                class="shrink-0 inline-flex items-center gap-1.5 rounded-md border border-neon-cyan/20 bg-neon-cyan/8 px-2 py-1 text-[10px] font-semibold uppercase tracking-[0.12em] text-neon-cyan transition-colors hover:bg-neon-cyan/14"
                                                on:click=on_apply
                                            >
                                                <Icon icon=LuPlay width="10px" height="10px" />
                                                "Load"
                                            </button>
                                        </div>
                                    </div>
                                </div>
                            </div>
                        }
                    }).collect_view()}
                </div>
            </div>

            <div>
                <div class="flex items-center gap-1.5 text-[10px] font-mono uppercase tracking-[0.14em] text-fg-tertiary/75 mb-2">
                    <Icon icon=LuLightbulb width="11px" height="11px" />
                    "Diagnosis Cheatsheet"
                </div>
                <div class="space-y-2">
                    {DIAGNOSIS_TIPS.into_iter().map(|tip| {
                        view! {
                            <div class="flex items-start gap-2 rounded-lg bg-black/10 px-2.5 py-2">
                                <span class="text-electric-yellow/80 mt-0.5 shrink-0">
                                    <Icon icon=tip.icon width="12px" height="12px" />
                                </span>
                                <div class="min-w-0">
                                    <div class="text-[11px] font-semibold text-fg-secondary">{tip.title}</div>
                                    <div class="text-[11px] leading-relaxed text-fg-tertiary mt-0.5">{tip.body}</div>
                                </div>
                            </div>
                        }
                    }).collect_view()}
                </div>
            </div>
        </div>
    }
}

#[component]
fn GuideChip(#[prop(into)] label: Signal<String>) -> impl IntoView {
    view! {
        <span class="inline-flex items-center rounded-full border border-white/8 bg-white/5 px-2 py-1 text-[10px] font-mono uppercase tracking-[0.12em] text-fg-tertiary/85">
            {move || label.get()}
        </span>
    }
}

fn control_label(values: &HashMap<String, ControlValue>, key: &str) -> Option<String> {
    values.get(key).and_then(|value| match value {
        ControlValue::Enum(label) | ControlValue::Text(label) => Some(label.clone()),
        _ => None,
    })
}

fn control_bool(values: &HashMap<String, ControlValue>, key: &str) -> bool {
    values
        .get(key)
        .is_some_and(|value| matches!(value, ControlValue::Boolean(true)))
}

fn control_number(values: &HashMap<String, ControlValue>, key: &str) -> Option<f32> {
    values.get(key).and_then(ControlValue::as_f32)
}

fn speed_feel(speed: f32) -> &'static str {
    match speed {
        speed if speed <= 12.0 => "very slow and deliberate",
        speed if speed <= 24.0 => "slow enough for placement work",
        speed if speed <= 45.0 => "medium-speed",
        _ => "fast",
    }
}

fn size_feel(size: f32) -> &'static str {
    match size {
        size if size <= 14.0 => "narrow marker",
        size if size <= 30.0 => "balanced marker",
        size if size <= 50.0 => "wide marker",
        _ => "very broad marker",
    }
}

fn calibration_focus(
    pattern: &str,
    direction: &str,
    grid_enabled: bool,
    speed: f32,
    size: f32,
) -> String {
    let pattern_help = match normalize_label(pattern).as_str() {
        "opposing_sweeps" => {
            "This pass is best for spotting mirrored mistakes and center alignment drift."
        }
        "crosshair" => {
            "This pass is best for precise X/Y placement because the intersection pinpoints where the layout thinks the hotspot lives."
        }
        "quadrant_cycle" => {
            "This pass is best for global orientation checks because each quadrant should read in the expected order."
        }
        "corner_cycle" => {
            "This pass is best for rotation and mirroring because the corner beacons should land exactly where your eyes expect."
        }
        "rings" => {
            "This pass is best for coverage and centering because rings make clipped or offset footprints obvious."
        }
        _ => "This pass is best for rough device placement and confirming overall sweep direction.",
    };

    format!(
        "{pattern} moving {direction} is {} with a {}. {pattern_help} {}",
        speed_feel(speed),
        size_feel(size),
        if grid_enabled {
            "Grid overlay is on, so you can also judge spacing against the canvas divisions."
        } else {
            "Turn on the grid overlay when you want a stronger read on spacing and centerlines."
        }
    )
}

fn next_step_for_pattern(pattern: &str) -> &'static str {
    match normalize_label(pattern).as_str() {
        "sweep" => "Vertical Sweep or Corner Compass",
        "opposing_sweeps" => "Corner Compass",
        "crosshair" => "Expanding Rings",
        "quadrant_cycle" => "Diagonal Crosshair",
        "corner_cycle" => "Quadrant Clock",
        "rings" => "Save the layout once centering feels right",
        _ => "Horizontal Sweep",
    }
}

fn normalize_label(value: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_separator = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            normalized.push('_');
            last_was_separator = true;
        }
    }

    normalized.trim_matches('_').to_owned()
}
