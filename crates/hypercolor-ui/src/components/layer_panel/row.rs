//! A single layer row and its transform/color sliders.

use std::collections::HashMap;

use hypercolor_leptos_ext::events::Change;
use hypercolor_types::layer::SceneLayer;
use leptos::prelude::*;
use leptos_icons::Icon;

use super::source::{
    blend_options, blend_value, fit_options, fit_value, layer_source_label, parse_blend, parse_fit,
};
use super::{delete_layer, reorder_layer, update_layer};
use crate::components::silk_select::SilkSelect;
use crate::icons::*;

/// One layer in the stack: identity, ordering controls, blend/opacity, and
/// a disclosure for transform and color adjustment.
#[component]
pub fn LayerRow(
    scene_id: String,
    group_id: String,
    layer: SceneLayer,
    stack_index: usize,
    total_layers: usize,
    stack: Vec<SceneLayer>,
    layers_version: u64,
    media_names: HashMap<String, String>,
    effect_names: HashMap<String, String>,
    on_layers_mutated: Callback<()>,
) -> impl IntoView {
    let source = layer_source_label(&layer.source, &media_names, &effect_names);
    let title = layer.name.clone().unwrap_or_else(|| source.clone());
    let layer_id = layer.id.to_string();
    let can_move_up = stack_index + 1 < total_layers;
    let can_move_down = stack_index > 0;
    let enabled = layer.enabled;
    let opacity = layer.opacity;
    let blend = layer.blend;
    let fit = layer.transform.fit;
    let brightness = layer.adjust.brightness;
    let saturation = layer.adjust.saturation;
    let tint_strength = layer.adjust.tint_strength;
    let scale_x = layer.transform.scale[0];
    let scale_y = layer.transform.scale[1];

    let update_enabled_layer = layer.clone();
    let update_blend_layer = layer.clone();
    let update_opacity_layer = layer.clone();
    let update_fit_layer = layer.clone();
    let update_brightness_layer = layer.clone();
    let update_saturation_layer = layer.clone();
    let update_tint_layer = layer.clone();
    let update_scale_x_layer = layer.clone();
    let update_scale_y_layer = layer.clone();
    let delete_layer_id = layer_id.clone();
    let move_up_stack = stack.clone();
    let move_down_stack = stack;
    let scene_enabled = scene_id.clone();
    let group_enabled = group_id.clone();
    let scene_blend = scene_id.clone();
    let group_blend = group_id.clone();
    let scene_opacity = scene_id.clone();
    let group_opacity = group_id.clone();
    let scene_fit = scene_id.clone();
    let group_fit = group_id.clone();
    let scene_brightness = scene_id.clone();
    let group_brightness = group_id.clone();
    let scene_saturation = scene_id.clone();
    let group_saturation = group_id.clone();
    let scene_tint = scene_id.clone();
    let group_tint = group_id.clone();
    let scene_scale_x = scene_id.clone();
    let group_scale_x = group_id.clone();
    let scene_scale_y = scene_id.clone();
    let group_scale_y = group_id.clone();
    let scene_delete = scene_id.clone();
    let group_delete = group_id.clone();
    let scene_move_up = scene_id.clone();
    let group_move_up = group_id.clone();
    let scene_move_down = scene_id.clone();
    let group_move_down = group_id.clone();
    let on_enabled = on_layers_mutated;
    let on_blend = on_layers_mutated;
    let on_opacity = on_layers_mutated;
    let on_fit = on_layers_mutated;
    let on_brightness = on_layers_mutated;
    let on_saturation = on_layers_mutated;
    let on_tint = on_layers_mutated;
    let on_scale_x = on_layers_mutated;
    let on_scale_y = on_layers_mutated;
    let on_delete = on_layers_mutated;
    let on_move_up = on_layers_mutated;
    let on_move_down = on_layers_mutated;

    view! {
        <article class="rounded-xl border border-edge-subtle/70 bg-surface-sunken/45 px-3 py-3 card-hover">
            <div class="flex items-start justify-between gap-2">
                <div class="min-w-0">
                    <div class="truncate text-sm font-semibold text-fg-primary">{title}</div>
                    <div class="mt-0.5 truncate text-[11px] text-fg-tertiary">{source}</div>
                </div>
                <div class="flex shrink-0 items-center gap-1">
                    <button
                        type="button"
                        class="rounded-md border border-edge-subtle p-1.5 text-fg-tertiary transition-colors hover:text-fg-primary disabled:opacity-30"
                        disabled=!can_move_up
                        on:click=move |_| reorder_layer(scene_move_up.clone(), group_move_up.clone(), move_up_stack.clone(), stack_index, 1, layers_version, on_move_up)
                    >
                        <Icon icon=LuChevronUp width="13px" height="13px" />
                    </button>
                    <button
                        type="button"
                        class="rounded-md border border-edge-subtle p-1.5 text-fg-tertiary transition-colors hover:text-fg-primary disabled:opacity-30"
                        disabled=!can_move_down
                        on:click=move |_| reorder_layer(scene_move_down.clone(), group_move_down.clone(), move_down_stack.clone(), stack_index, -1, layers_version, on_move_down)
                    >
                        <Icon icon=LuChevronDown width="13px" height="13px" />
                    </button>
                    <button
                        type="button"
                        class="rounded-md border border-red-400/20 p-1.5 text-red-300 transition-colors hover:bg-red-400/10 btn-press"
                        on:click=move |_| delete_layer(scene_delete.clone(), group_delete.clone(), delete_layer_id.clone(), layers_version, on_delete)
                    >
                        <Icon icon=LuTrash2 width="13px" height="13px" />
                    </button>
                </div>
            </div>

            <div class="mt-3 grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-2">
                <label class="flex items-center gap-2 text-[11px] text-fg-secondary">
                    <input
                        type="checkbox"
                        class="accent-accent"
                        prop:checked=enabled
                        on:change=move |event| {
                            if let Some(checked) = Change::from_event(event).checked() {
                                let mut next = update_enabled_layer.clone();
                                next.enabled = checked;
                                update_layer(scene_enabled.clone(), group_enabled.clone(), next, layers_version, on_enabled);
                            }
                        }
                    />
                    "Enabled"
                </label>
                <SilkSelect
                    value=Signal::derive(move || blend_value(blend).to_owned())
                    options=Signal::derive(blend_options)
                    on_change=Callback::new(move |value: String| {
                        let mut next = update_blend_layer.clone();
                        next.blend = parse_blend(&value);
                        update_layer(scene_blend.clone(), group_blend.clone(), next, layers_version, on_blend);
                    })
                    placeholder="Blend"
                    class="border border-edge-subtle bg-surface-overlay/45 px-2.5 py-1.5 text-[11px] text-fg-primary"
                    label_class="font-mono"
                />
                <span class="text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/70">"Opacity"</span>
                <input
                    type="range"
                    min="0"
                    max="1"
                    step="0.01"
                    class="w-full accent-accent"
                    prop:value=format!("{opacity:.2}")
                    on:change=move |event| {
                        if let Some(value) = Change::from_event(event).value::<f32>() {
                            let mut next = update_opacity_layer.clone();
                            next.opacity = value.clamp(0.0, 1.0);
                            update_layer(scene_opacity.clone(), group_opacity.clone(), next, layers_version, on_opacity);
                        }
                    }
                />
            </div>

            <details class="mt-3 rounded-lg border border-edge-subtle/60 bg-surface-overlay/25">
                <summary class="cursor-pointer px-3 py-2 text-[11px] font-semibold text-fg-secondary">
                    "Transform & Color"
                </summary>
                <div class="space-y-3 border-t border-edge-subtle/50 px-3 py-3">
                    <SilkSelect
                        value=Signal::derive(move || fit_value(fit).to_owned())
                        options=Signal::derive(fit_options)
                        on_change=Callback::new(move |value: String| {
                            let mut next = update_fit_layer.clone();
                            next.transform.fit = parse_fit(&value);
                            update_layer(scene_fit.clone(), group_fit.clone(), next, layers_version, on_fit);
                        })
                        placeholder="Fit"
                        class="border border-edge-subtle bg-surface-sunken/55 px-2.5 py-1.5 text-[11px] text-fg-primary"
                        label_class="font-mono"
                    />
                    <LayerSlider
                        label="Brightness"
                        value=brightness
                        min=0.0
                        max=4.0
                        step=0.05
                        on_change=Callback::new(move |value: f32| {
                            let mut next = update_brightness_layer.clone();
                            next.adjust.brightness = value.clamp(0.0, 4.0);
                            update_layer(scene_brightness.clone(), group_brightness.clone(), next, layers_version, on_brightness);
                        })
                    />
                    <LayerSlider
                        label="Saturation"
                        value=saturation
                        min=0.0
                        max=4.0
                        step=0.05
                        on_change=Callback::new(move |value: f32| {
                            let mut next = update_saturation_layer.clone();
                            next.adjust.saturation = value.clamp(0.0, 4.0);
                            update_layer(scene_saturation.clone(), group_saturation.clone(), next, layers_version, on_saturation);
                        })
                    />
                    <LayerSlider
                        label="Tint"
                        value=tint_strength
                        min=0.0
                        max=1.0
                        step=0.01
                        on_change=Callback::new(move |value: f32| {
                            let mut next = update_tint_layer.clone();
                            next.adjust.tint_strength = value.clamp(0.0, 1.0);
                            update_layer(scene_tint.clone(), group_tint.clone(), next, layers_version, on_tint);
                        })
                    />
                    <LayerSlider
                        label="Scale X"
                        value=scale_x
                        min=0.1
                        max=4.0
                        step=0.05
                        on_change=Callback::new(move |value: f32| {
                            let mut next = update_scale_x_layer.clone();
                            next.transform.scale[0] = value.clamp(0.1, 4.0);
                            update_layer(scene_scale_x.clone(), group_scale_x.clone(), next, layers_version, on_scale_x);
                        })
                    />
                    <LayerSlider
                        label="Scale Y"
                        value=scale_y
                        min=0.1
                        max=4.0
                        step=0.05
                        on_change=Callback::new(move |value: f32| {
                            let mut next = update_scale_y_layer.clone();
                            next.transform.scale[1] = value.clamp(0.1, 4.0);
                            update_layer(scene_scale_y.clone(), group_scale_y.clone(), next, layers_version, on_scale_y);
                        })
                    />
                </div>
            </details>
        </article>
    }
}

#[component]
fn LayerSlider(
    label: &'static str,
    value: f32,
    min: f32,
    max: f32,
    step: f32,
    on_change: Callback<f32>,
) -> impl IntoView {
    view! {
        <label class="grid grid-cols-[78px_1fr_38px] items-center gap-2 text-[10px] font-mono text-fg-tertiary/75">
            <span class="uppercase tracking-wide">{label}</span>
            <input
                type="range"
                min=min.to_string()
                max=max.to_string()
                step=step.to_string()
                class="w-full accent-accent"
                prop:value=format!("{value:.2}")
                on:change=move |event| {
                    if let Some(value) = Change::from_event(event).value::<f32>() {
                        on_change.run(value);
                    }
                }
            />
            <span class="text-right">{format!("{value:.2}")}</span>
        </label>
    }
}
