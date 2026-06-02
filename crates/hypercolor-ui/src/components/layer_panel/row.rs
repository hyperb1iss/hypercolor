//! A single layer card — the unit of the layer inspector.
//!
//! The card leads with the layer's identity, then its compositing
//! controls (enable, blend, opacity), then the source's own controls —
//! an effect's parameters or a media clip's playback — and finally the
//! transform/color disclosure. With one layer in the stack the reorder
//! affordances are suppressed: a single layer has nowhere to move.

use std::collections::HashMap;

use hypercolor_leptos_ext::events::Change;
use hypercolor_types::event::LayerHealth;
use hypercolor_types::layer::{LayerSource, SceneLayer};
use leptos::prelude::*;
use leptos_icons::Icon;

use super::controls::{EffectControlsSection, LayerToggleTrack, MediaPlaybackSection};
use super::source::{blend_options, blend_value, fit_options, fit_value, parse_blend, parse_fit};
use super::{delete_layer, reorder_layer, update_layer};
use crate::components::silk_select::SilkSelect;
use crate::icons::*;

/// Icon, accent RGB triplet, and kind word for a layer source — the
/// header chip's vocabulary. Never an internal enum name (§4).
fn source_meta(source: &LayerSource) -> (icondata_core::Icon, &'static str, &'static str) {
    match source {
        LayerSource::Effect { .. } => (LuZap, "225, 53, 255", "Effect"),
        LayerSource::Media { .. } => (LuFolder, "128, 255, 234", "Media"),
        LayerSource::ScreenRegion { .. } => (LuMonitor, "241, 250, 140", "Screen capture"),
        LayerSource::WebViewport { .. } => (LuGlobe, "130, 170, 255", "Web page"),
        LayerSource::ColorFill { .. } => (LuPalette, "255, 106, 193", "Color"),
    }
}

/// The display title for a layer: the user's typed name, else the
/// resolved content name (effect / media), else the kind word.
fn layer_title(
    layer: &SceneLayer,
    media_names: &HashMap<String, String>,
    effect_names: &HashMap<String, String>,
    kind_word: &str,
) -> String {
    if let Some(name) = layer.name.as_ref().filter(|name| !name.trim().is_empty()) {
        return name.clone();
    }
    match &layer.source {
        LayerSource::Effect { effect_id, .. } => effect_names
            .get(&effect_id.to_string())
            .cloned()
            .unwrap_or_else(|| kind_word.to_owned()),
        LayerSource::Media { asset_id, .. } => media_names
            .get(&asset_id.to_string())
            .cloned()
            .unwrap_or_else(|| kind_word.to_owned()),
        LayerSource::WebViewport { url, .. } => url.clone(),
        LayerSource::ScreenRegion { .. } | LayerSource::ColorFill { .. } => kind_word.to_owned(),
    }
}

/// One layer in the stack: identity, ordering controls, blend/opacity,
/// the source's own controls, and a transform/color disclosure.
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
    #[prop(into)] health: Signal<Option<LayerHealth>>,
    on_layers_mutated: Callback<()>,
) -> impl IntoView {
    let (icon, accent_rgb, kind_word) = source_meta(&layer.source);
    let title = layer_title(&layer, &media_names, &effect_names, kind_word);
    let layer_id = layer.id.to_string();
    let is_effect = matches!(layer.source, LayerSource::Effect { .. });
    let is_media = matches!(layer.source, LayerSource::Media { .. });
    // A lone layer has nowhere to move; reorder appears only in a stack.
    let show_reorder = total_layers > 1;
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

    let enabled_layer = layer.clone();
    let blend_layer = layer.clone();
    let opacity_layer = layer.clone();
    let fit_layer = layer.clone();
    let brightness_layer = layer.clone();
    let saturation_layer = layer.clone();
    let tint_layer = layer.clone();
    let scale_x_layer = layer.clone();
    let scale_y_layer = layer.clone();
    let effect_layer = layer.clone();
    let media_layer = layer.clone();
    let move_up_stack = stack.clone();
    let move_down_stack = stack;

    let chip_style = format!("background: rgba({accent_rgb}, 0.14)");
    let icon_style = format!("color: rgb({accent_rgb})");

    let toggle_enabled = {
        let scene_id = scene_id.clone();
        let group_id = group_id.clone();
        move |_| {
            let mut next = enabled_layer.clone();
            next.enabled = !next.enabled;
            update_layer(
                scene_id.clone(),
                group_id.clone(),
                next,
                layers_version,
                on_layers_mutated,
            );
        }
    };

    view! {
        <article class="overflow-hidden rounded-xl border border-edge-subtle/70 bg-surface-sunken/50 transition-colors duration-150 hover:border-edge-subtle">
            // ── Header: identity + ordering + delete ──────────────────
            <div class="flex items-center gap-2.5 px-3 py-2.5">
                <span
                    class="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg"
                    style=chip_style
                >
                    <Icon icon=icon width="15px" height="15px" style=icon_style />
                </span>
                <div class="min-w-0 flex-1">
                    <div class="flex items-center gap-1.5">
                        <span class="min-w-0 truncate text-sm font-semibold text-fg-primary">
                            {title}
                        </span>
                        {move || health_pill(health.get())}
                    </div>
                    <span class="text-[10px] font-mono uppercase tracking-[0.12em] text-fg-tertiary/65">
                        {kind_word}
                    </span>
                </div>
                <div class="flex shrink-0 items-center gap-1">
                    {show_reorder
                        .then(|| {
                            let scene_up = scene_id.clone();
                            let group_up = group_id.clone();
                            let scene_down = scene_id.clone();
                            let group_down = group_id.clone();
                            let up_stack = move_up_stack.clone();
                            let down_stack = move_down_stack.clone();
                            view! {
                                <button
                                    type="button"
                                    class="rounded-md p-1.5 text-fg-tertiary transition-colors hover:text-fg-primary disabled:opacity-25"
                                    disabled=!can_move_up
                                    title="Move layer up"
                                    on:click=move |_| reorder_layer(
                                        scene_up.clone(),
                                        group_up.clone(),
                                        up_stack.clone(),
                                        stack_index,
                                        1,
                                        layers_version,
                                        on_layers_mutated,
                                    )
                                >
                                    <Icon icon=LuChevronUp width="14px" height="14px" />
                                </button>
                                <button
                                    type="button"
                                    class="rounded-md p-1.5 text-fg-tertiary transition-colors hover:text-fg-primary disabled:opacity-25"
                                    disabled=!can_move_down
                                    title="Move layer down"
                                    on:click=move |_| reorder_layer(
                                        scene_down.clone(),
                                        group_down.clone(),
                                        down_stack.clone(),
                                        stack_index,
                                        -1,
                                        layers_version,
                                        on_layers_mutated,
                                    )
                                >
                                    <Icon icon=LuChevronDown width="14px" height="14px" />
                                </button>
                            }
                        })}
                    <button
                        type="button"
                        class="rounded-md p-1.5 text-fg-tertiary transition-colors hover:text-status-error btn-press"
                        title="Delete layer"
                        on:click={
                            let scene_id = scene_id.clone();
                            let group_id = group_id.clone();
                            let layer_id = layer_id.clone();
                            move |_| delete_layer(
                                scene_id.clone(),
                                group_id.clone(),
                                layer_id.clone(),
                                layers_version,
                                on_layers_mutated,
                            )
                        }
                    >
                        <Icon icon=LuTrash2 width="14px" height="14px" />
                    </button>
                </div>
            </div>

            <div class="space-y-3 border-t border-edge-subtle/45 px-3 py-3">
                // ── Enable + blend ────────────────────────────────────
                <div class="flex items-center gap-2">
                    <button
                        type="button"
                        class="flex items-center gap-2 rounded-lg border border-edge-subtle/55 bg-surface-overlay/35 px-2.5 py-1.5 text-xs font-medium text-fg-secondary transition-colors duration-200 hover:border-accent-muted/50"
                        on:click=toggle_enabled
                    >
                        <LayerToggleTrack on=enabled />
                        {if enabled { "On" } else { "Off" }}
                    </button>
                    <div class="min-w-0 flex-1">
                        <SilkSelect
                            value=Signal::derive(move || blend_value(blend).to_owned())
                            options=Signal::derive(blend_options)
                            on_change=Callback::new({
                                let scene_id = scene_id.clone();
                                let group_id = group_id.clone();
                                move |value: String| {
                                    let mut next = blend_layer.clone();
                                    next.blend = parse_blend(&value);
                                    update_layer(
                                        scene_id.clone(),
                                        group_id.clone(),
                                        next,
                                        layers_version,
                                        on_layers_mutated,
                                    );
                                }
                            })
                            placeholder="Blend"
                            class="w-full border border-edge-subtle bg-surface-overlay/45 px-2.5 py-1.5 text-xs text-fg-primary"
                            label_class="font-medium"
                        />
                    </div>
                </div>

                // ── Opacity — same slider chrome as the Effects controls ──
                <div
                    class="flex items-center gap-2.5 rounded-lg px-3 py-2 transition-colors duration-200 hover:bg-surface-hover/20"
                    style="--glow-rgb: 225, 53, 255"
                >
                    <span class="min-w-[64px] shrink-0 truncate text-xs font-medium text-fg-secondary">
                        "Opacity"
                    </span>
                    <input
                        type="range"
                        min="0"
                        max="1"
                        step="0.01"
                        class="slider-silk min-w-0 flex-1 cursor-pointer"
                        prop:value=format!("{opacity:.2}")
                        on:change={
                            let scene_id = scene_id.clone();
                            let group_id = group_id.clone();
                            move |event| {
                                if let Some(value) = Change::from_event(event).value::<f32>() {
                                    let mut next = opacity_layer.clone();
                                    next.opacity = value.clamp(0.0, 1.0);
                                    update_layer(
                                        scene_id.clone(),
                                        group_id.clone(),
                                        next,
                                        layers_version,
                                        on_layers_mutated,
                                    );
                                }
                            }
                        }
                    />
                    <span
                        class="w-[40px] shrink-0 rounded px-1.5 py-0.5 text-right font-mono text-[10px] tabular-nums"
                        style="color: rgba(225, 53, 255, 0.85); background: rgba(225, 53, 255, 0.1)"
                    >
                        {format!("{:.0}%", opacity * 100.0)}
                    </span>
                </div>

                // ── Source-specific controls ──────────────────────────
                {is_effect
                    .then(|| {
                        view! {
                            <div class="border-t border-edge-subtle/40 pt-3">
                                <EffectControlsSection
                                    scene_id=scene_id.clone()
                                    group_id=group_id.clone()
                                    layer=effect_layer.clone()
                                    layers_version=layers_version
                                />
                            </div>
                        }
                    })}
                {is_media
                    .then(|| {
                        view! {
                            <div class="border-t border-edge-subtle/40 pt-3">
                                <MediaPlaybackSection
                                    scene_id=scene_id.clone()
                                    group_id=group_id.clone()
                                    layer=media_layer.clone()
                                    layers_version=layers_version
                                    on_layers_mutated=on_layers_mutated
                                />
                            </div>
                        }
                    })}

                // ── Transform & color disclosure ──────────────────────
                <details class="rounded-lg border border-edge-subtle/55 bg-surface-overlay/25">
                    <summary class="flex cursor-pointer items-center gap-1.5 px-3 py-2 text-[11px] font-semibold text-fg-secondary">
                        <Icon icon=LuChevronRight width="12px" height="12px" />
                        "Transform & Color"
                    </summary>
                    <div class="space-y-3 border-t border-edge-subtle/45 px-3 py-3">
                        <SilkSelect
                            value=Signal::derive(move || fit_value(fit).to_owned())
                            options=Signal::derive(fit_options)
                            on_change=Callback::new({
                                let scene_id = scene_id.clone();
                                let group_id = group_id.clone();
                                move |value: String| {
                                    let mut next = fit_layer.clone();
                                    next.transform.fit = parse_fit(&value);
                                    update_layer(
                                        scene_id.clone(),
                                        group_id.clone(),
                                        next,
                                        layers_version,
                                        on_layers_mutated,
                                    );
                                }
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
                            on_change=Callback::new({
                                let scene_id = scene_id.clone();
                                let group_id = group_id.clone();
                                move |value: f32| {
                                    let mut next = brightness_layer.clone();
                                    next.adjust.brightness = value.clamp(0.0, 4.0);
                                    update_layer(
                                        scene_id.clone(),
                                        group_id.clone(),
                                        next,
                                        layers_version,
                                        on_layers_mutated,
                                    );
                                }
                            })
                        />
                        <LayerSlider
                            label="Saturation"
                            value=saturation
                            min=0.0
                            max=4.0
                            step=0.05
                            on_change=Callback::new({
                                let scene_id = scene_id.clone();
                                let group_id = group_id.clone();
                                move |value: f32| {
                                    let mut next = saturation_layer.clone();
                                    next.adjust.saturation = value.clamp(0.0, 4.0);
                                    update_layer(
                                        scene_id.clone(),
                                        group_id.clone(),
                                        next,
                                        layers_version,
                                        on_layers_mutated,
                                    );
                                }
                            })
                        />
                        <LayerSlider
                            label="Tint"
                            value=tint_strength
                            min=0.0
                            max=1.0
                            step=0.01
                            on_change=Callback::new({
                                let scene_id = scene_id.clone();
                                let group_id = group_id.clone();
                                move |value: f32| {
                                    let mut next = tint_layer.clone();
                                    next.adjust.tint_strength = value.clamp(0.0, 1.0);
                                    update_layer(
                                        scene_id.clone(),
                                        group_id.clone(),
                                        next,
                                        layers_version,
                                        on_layers_mutated,
                                    );
                                }
                            })
                        />
                        <LayerSlider
                            label="Scale X"
                            value=scale_x
                            min=0.1
                            max=4.0
                            step=0.05
                            on_change=Callback::new({
                                let scene_id = scene_id.clone();
                                let group_id = group_id.clone();
                                move |value: f32| {
                                    let mut next = scale_x_layer.clone();
                                    next.transform.scale[0] = value.clamp(0.1, 4.0);
                                    update_layer(
                                        scene_id.clone(),
                                        group_id.clone(),
                                        next,
                                        layers_version,
                                        on_layers_mutated,
                                    );
                                }
                            })
                        />
                        <LayerSlider
                            label="Scale Y"
                            value=scale_y
                            min=0.1
                            max=4.0
                            step=0.05
                            on_change=Callback::new({
                                let scene_id = scene_id.clone();
                                let group_id = group_id.clone();
                                move |value: f32| {
                                    let mut next = scale_y_layer.clone();
                                    next.transform.scale[1] = value.clamp(0.1, 4.0);
                                    update_layer(
                                        scene_id.clone(),
                                        group_id.clone(),
                                        next,
                                        layers_version,
                                        on_layers_mutated,
                                    );
                                }
                            })
                        />
                    </div>
                </details>
            </div>
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
        <label class="grid grid-cols-[64px_1fr_44px] items-center gap-2 text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/75">
            <span>{label}</span>
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
            <span class="text-right tabular-nums">{format!("{value:.2}")}</span>
        </label>
    }
}

/// A small status pill for a layer's runtime health. A healthy (`Active`)
/// or not-yet-reported layer shows nothing — the pill flags only trouble.
fn health_pill(health: Option<LayerHealth>) -> impl IntoView {
    health.and_then(|health| {
        let (label, classes, tooltip): (&str, &str, String) = match health {
            LayerHealth::Active => return None,
            LayerHealth::Loading => (
                "Loading",
                "border-status-info/30 bg-status-info/10 text-status-info",
                "Layer is still loading".to_owned(),
            ),
            LayerHealth::Stalled => (
                "Stalled",
                "border-status-warning/30 bg-status-warning/10 text-status-warning",
                "Layer producer has stalled".to_owned(),
            ),
            LayerHealth::AssetMissing => (
                "Missing",
                "border-status-error/30 bg-status-error/10 text-status-error",
                "Layer asset is missing".to_owned(),
            ),
            LayerHealth::Failed { reason } => (
                "Failed",
                "border-status-error/30 bg-status-error/10 text-status-error",
                format!("Layer failed: {reason}"),
            ),
        };
        Some(view! {
            <span
                class=format!(
                    "shrink-0 rounded-full border px-1.5 py-0.5 text-[9px] font-semibold \
                     uppercase tracking-wide {classes}",
                )
                title=tooltip
            >
                {label}
            </span>
        })
    })
}
