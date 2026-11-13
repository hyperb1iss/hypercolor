//! A single layer card — the unit of the layer inspector.
//!
//! The card leads with the layer's identity, then its compositing
//! controls (blend and opacity), then the source's own controls —
//! an effect's parameters or a media clip's playback — and finally the
//! transform/color disclosure. With one layer in the stack the reorder
//! affordances are suppressed: a single layer has nowhere to move.
//!
//! Under the name the card says what the layer applies to, when its host
//! can show that: "Whole zone" in plain text, a chip naming its outputs,
//! or a warning once they have all left the zone. Each opens the target
//! editor. Hovering the identity lights the layer's outputs on the host's
//! canvas, and the card answers the canvas pointer the same way.

use std::collections::{HashMap, HashSet};

use hypercolor_leptos_ext::events::Change;
use hypercolor_types::event::LayerHealth;
use hypercolor_types::layer::{LayerSource, SceneLayer};
use leptos::prelude::*;
use leptos_icons::Icon;

use super::controls::{EffectControlsSection, MediaPlaybackSection};
use super::scope::{
    TargetWords, hover_outputs, names_output, offers_targets, outputs_detail, stands_back,
    target_words,
};
use super::source::{blend_options, blend_value, fit_options, fit_value, parse_blend, parse_fit};
use super::{
    LayerTargetHost, LayerWrites, TargetSubject, delete_layer, reorder_layer, update_layer,
};
use crate::components::silk_select::SilkSelect;
use crate::icons::*;
use crate::pointer_gesture::listener_element;

/// What a row says its layer applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RowTarget {
    words: TargetWords,
    /// Every output by its full name, for the tooltip.
    detail: String,
}

/// The dark pill a layer's target shows in.
const TARGET_CHIP: &str = "group/target inline-flex min-w-0 max-w-[13rem] items-center gap-1 rounded-full border \
     border-edge-subtle bg-surface-overlay py-0.5 pl-1.5 pr-1 text-[10px] font-medium \
     text-fg-secondary transition-colors hover:border-accent-muted hover:text-fg-primary";

/// Icon, accent RGB triplet, and kind word for a layer source — the
/// header chip's vocabulary. Never an internal enum name (§4).
#[must_use]
pub fn source_meta(source: &LayerSource) -> (icondata_core::Icon, &'static str, &'static str) {
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
#[must_use]
pub fn layer_title(
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
    zone_id: String,
    layer: Signal<SceneLayer>,
    stack_index: Signal<usize>,
    fence: Signal<crate::api::Fence>,
    media_names: Memo<HashMap<String, String>>,
    effect_names: Memo<HashMap<String, String>>,
    effect_cache: super::LayerEffectCache,
    expanded: bool,
    on_disclosure: Callback<bool>,
    #[prop(into)] health: Signal<Option<LayerHealth>>,
    writes: LayerWrites,
    /// The host's outputs and canvas, when it shows what a layer applies
    /// to.
    #[prop(into)]
    targets: Signal<Option<LayerTargetHost>>,
) -> impl IntoView {
    let initial = layer.get_untracked();
    let stack = writes.stack;
    let on_layers_mutated = writes.mutated;
    let (icon, accent_rgb, kind_word) = source_meta(&initial.source);
    let title = Memo::new(move |_| {
        layer_title(
            &layer.get(),
            &media_names.get(),
            &effect_names.get(),
            kind_word,
        )
    });

    // What the layer applies to. A zone with nothing to choose between
    // says nothing, unless the layer is limited anyway.
    let target = Memo::new(move |_| {
        let host = targets.get()?;
        let scope = layer.with(|layer| layer.scope.clone());
        host.outputs.with(|outputs| {
            if scope.is_none() && !offers_targets(outputs) {
                return None;
            }
            let ids: HashSet<&str> = scope
                .iter()
                .flat_map(|scope| scope.outputs.iter().map(String::as_str))
                .collect();
            Some(RowTarget {
                words: target_words(scope.as_ref(), outputs),
                detail: outputs_detail(&ids, outputs),
            })
        })
    });
    let recedes = Memo::new(move |_| {
        targets.get().is_some_and(|host| {
            host.emphasis
                .with(|selected| layer.with(|layer| stands_back(layer, selected)))
        })
    });
    let answers_pointer = Memo::new(move |_| {
        targets.get().is_some_and(|host| {
            host.pointer
                .with(|pointer| layer.with(|layer| names_output(layer, pointer.as_deref())))
        })
    });
    // The canvas highlight this row raised must not outlive it.
    let lighting = StoredValue::new(false);
    let light = move |on: bool| {
        let Some(host) = targets.get_untracked() else {
            return;
        };
        let outputs = if on {
            host.outputs.with_untracked(|outputs| {
                let members: HashSet<&str> =
                    outputs.iter().map(|output| output.id.as_str()).collect();
                hover_outputs(&layer.get_untracked(), &members)
            })
        } else {
            HashSet::new()
        };
        if on || lighting.get_value() {
            lighting.set_value(on && !outputs.is_empty());
            host.hover.run(outputs);
        }
    };
    on_cleanup(move || {
        if lighting.try_get_value() == Some(true)
            && let Some(host) = targets.try_get_untracked().flatten()
        {
            let _ = host.hover.try_run(HashSet::new());
        }
    });
    let edit_target = move |ev: web_sys::MouseEvent| {
        ev.stop_propagation();
        if let (Some(host), Some(element)) = (targets.get_untracked(), listener_element(&ev)) {
            host.edit.run((
                TargetSubject::Layer {
                    id: layer.get_untracked().id,
                    name: title.get_untracked(),
                },
                element,
            ));
        }
    };
    let layer_id = initial.id.to_string();
    let is_effect = matches!(initial.source, LayerSource::Effect { .. });
    let is_media = matches!(initial.source, LayerSource::Media { .. });
    let show_reorder = move || stack.with(|layers| layers.len() > 1);
    let can_move_up = move || stack_index.get() + 1 < stack.with(Vec::len);
    let can_move_down = move || stack_index.get() > 0;

    let dormant_zone_id = zone_id.clone();
    let dormant_layer_id = layer_id.clone();

    let chip_style = format!("background: rgba({accent_rgb}, 0.14)");
    let icon_style = format!("color: rgb({accent_rgb})");

    view! {
        <article
            class="overflow-hidden rounded-xl border border-edge-subtle/70 bg-surface-sunken/50 transition-[border-color,opacity] duration-150 hover:border-edge-subtle"
            class=("opacity-50", move || recedes.get())
            class=("hover:opacity-100", move || recedes.get())
            style:border-color=move || {
                answers_pointer
                    .get()
                    .then_some("color-mix(in oklch, var(--color-accent) 60%, transparent)")
            }
            data-layer-row=layer_id.clone()
            data-layer-recedes=move || recedes.get().to_string()
        >
            // ── Header: identity + ordering + delete ──────────────────
            // A grid, so the line under the name runs the card's full width
            // beneath the ordering buttons: a target's words need the room
            // more than the buttons need the height.
            <div
                class="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-2.5 px-3 py-2.5"
                on:mouseenter=move |_| light(true)
                on:mouseleave=move |_| light(false)
            >
                <span
                    class="row-span-2 flex h-8 w-8 shrink-0 items-center justify-center rounded-lg"
                    style=chip_style
                >
                    <Icon icon=icon width="15px" height="15px" style=icon_style />
                </span>
                    <div class="flex min-w-0 items-center gap-1.5">
                        // A row the selection does not reach keeps its place
                        // and is marked, so the stack still reads in order.
                        <Show when=move || recedes.get()>
                            <span
                                class="h-1 w-1 shrink-0 rounded-full bg-fg-tertiary"
                                title="Does not apply to the selected output"
                            />
                        </Show>
                        <span class="min-w-0 truncate text-sm font-semibold text-fg-primary">
                            {move || title.get()}
                        </span>
                        {move || health_pill(health.get())}
                    </div>
                    <div class="col-span-2 col-start-2 row-start-2 flex min-w-0 items-center gap-1.5">
                        <span class="shrink-0 text-[10px] font-mono uppercase tracking-[0.12em] text-fg-tertiary/65">
                            {kind_word}
                        </span>
                        {move || {
                            let RowTarget { words, detail } = target.get()?;
                            Some(match words {
                                TargetWords::WholeZone => view! {
                                    <button
                                        type="button"
                                        class="min-w-0 truncate rounded px-1 text-[10px] text-fg-tertiary transition-colors hover:bg-surface-hover/40 hover:text-fg-secondary"
                                        aria-haspopup="dialog"
                                        aria-label=move || format!("{} applies to the whole zone", title.get())
                                        title="Choose what this layer applies to"
                                        on:click=edit_target
                                    >
                                        {words.label().to_owned()}
                                    </button>
                                }
                                .into_any(),
                                TargetWords::Outputs { ref label, missing } => {
                                    let label = label.clone();
                                    let tip = if missing == 0 {
                                        format!("Applies to {detail}")
                                    } else {
                                        format!(
                                            "Applies to {detail}. {missing} more {} not in this zone.",
                                            if missing == 1 { "output is" } else { "outputs are" },
                                        )
                                    };
                                    let named = label.clone();
                                    view! {
                                        <button
                                            type="button"
                                            class=TARGET_CHIP
                                            aria-haspopup="dialog"
                                            aria-label=move || format!("{} applies to {named}", title.get())
                                            title=tip
                                            data-layer-target=""
                                            on:click=edit_target
                                        >
                                            <span class="flex shrink-0 text-accent">
                                                <Icon icon=LuScan width="10px" height="10px" />
                                            </span>
                                            <span class="min-w-0 truncate">{label}</span>
                                            {(missing > 0).then(|| view! {
                                                <span class="flex shrink-0 text-status-warning">
                                                    <Icon icon=LuTriangleAlert width="10px" height="10px" />
                                                </span>
                                            })}
                                            <span class="flex shrink-0 text-fg-tertiary transition-colors group-hover/target:text-fg-secondary">
                                                <Icon icon=LuChevronDown width="10px" height="10px" />
                                            </span>
                                        </button>
                                    }
                                    .into_any()
                                }
                                TargetWords::Dormant => {
                                    let zone_id = dormant_zone_id.clone();
                                    let layer_id = dormant_layer_id.clone();
                                    view! {
                                        <button
                                            type="button"
                                            class="inline-flex min-w-0 items-center gap-1 rounded-full border border-status-warning/30 bg-status-warning/10 px-1.5 py-0.5 text-[10px] font-medium text-status-warning transition-colors hover:border-status-warning/60"
                                            aria-haspopup="dialog"
                                            aria-label=move || format!("{} applies to outputs that are not in this zone", title.get())
                                            title="Its outputs have left this zone, so it draws nothing. Choose other outputs, or remove it."
                                            data-layer-target="dormant"
                                            on:click=edit_target
                                        >
                                            <span class="flex shrink-0">
                                                <Icon icon=LuTriangleAlert width="10px" height="10px" />
                                            </span>
                                            <span class="min-w-0 truncate">{words.label().to_owned()}</span>
                                        </button>
                                        <button
                                            type="button"
                                            class="shrink-0 rounded px-1 text-[10px] font-medium text-fg-tertiary transition-colors hover:bg-status-error/10 hover:text-status-error"
                                            aria-label=move || format!("Remove {}", title.get())
                                            on:click=move |_| delete_layer(
                                                zone_id.clone(),
                                                layer_id.clone(),
                                                fence.get_untracked(),
                                                on_layers_mutated,
                                            )
                                        >
                                            "Remove"
                                        </button>
                                    }
                                    .into_any()
                                }
                            })
                        }}
                    </div>
                <div class="col-start-3 row-start-1 flex shrink-0 items-center gap-1">
                    {let zone_id = zone_id.clone(); move || show_reorder()
                        .then(|| {
                            let zone_up = zone_id.clone();
                            let zone_down = zone_id.clone();
                            let up_stack = stack;
                            let down_stack = stack;
                            view! {
                                <button
                                    type="button"
                                    class="rounded-md p-1.5 text-fg-tertiary transition-colors hover:text-fg-primary disabled:opacity-25"
                                    disabled=move || !can_move_up()
                                    title="Move layer up"
                                    on:click=move |_| reorder_layer(
                                        zone_up.clone(),
                                        up_stack.get_untracked(),
                                        stack_index.get_untracked(),
                                        1,
                                        fence.get_untracked(),
                                        on_layers_mutated,
                                    )
                                >
                                    <Icon icon=LuChevronUp width="14px" height="14px" />
                                </button>
                                <button
                                    type="button"
                                    class="rounded-md p-1.5 text-fg-tertiary transition-colors hover:text-fg-primary disabled:opacity-25"
                                    disabled=move || !can_move_down()
                                    title="Move layer down"
                                    on:click=move |_| reorder_layer(
                                        zone_down.clone(),
                                        down_stack.get_untracked(),
                                        stack_index.get_untracked(),
                                        -1,
                                        fence.get_untracked(),
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
                            let zone_id = zone_id.clone();
                            let layer_id = layer_id.clone();
                            move |_| delete_layer(
                                zone_id.clone(),
                                layer_id.clone(),
                                fence.get_untracked(),
                                on_layers_mutated,
                            )
                        }
                    >
                        <Icon icon=LuTrash2 width="14px" height="14px" />
                    </button>
                </div>
            </div>

            <div class="space-y-3 border-t border-edge-subtle/45 px-3 py-3">
                // ── Blend ─────────────────────────────────────────────
                <div class="min-w-0">
                        <SilkSelect
                            aria_label="Blend mode"
                            value=Signal::derive(move || blend_value(layer.get().blend).to_owned())
                            options=Signal::derive(blend_options)
                            on_change=Callback::new({
                                let zone_id = zone_id.clone();
                                move |value: String| {
                                    let mut next = layer.get_untracked();
                                    next.blend = parse_blend(&value);
                                    update_layer(
                                        zone_id.clone(),
                                        next,
                                        fence.get_untracked(),
                                        writes,
                                    );
                                }
                            })
                            placeholder="Blend"
                            class="w-full border border-edge-subtle bg-surface-overlay/45 px-2.5 py-1.5 text-xs text-fg-primary"
                            label_class="font-medium"
                        />
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
                        prop:value=move || format!("{:.2}", layer.get().opacity)
                        on:change={
                            let zone_id = zone_id.clone();
                            move |event| {
                                if let Some(value) = Change::from_event(event).value::<f32>() {
                                    let mut next = layer.get_untracked();
                                    next.opacity = value.clamp(0.0, 1.0);
                                    update_layer(
                                        zone_id.clone(),
                                        next,
                                        fence.get_untracked(),
                                        writes,
                                    );
                                }
                            }
                        }
                    />
                    <span
                        class="w-[40px] shrink-0 rounded px-1.5 py-0.5 text-right font-mono text-[10px] tabular-nums"
                        style="color: rgba(225, 53, 255, 0.85); background: rgba(225, 53, 255, 0.1)"
                    >
                        {move || format!("{:.0}%", layer.get().opacity * 100.0)}
                    </span>
                </div>

                // ── Source-specific controls ──────────────────────────
                {is_effect
                    .then(|| {
                        view! {
                            <div class="border-t border-edge-subtle/40 pt-3">
                                <EffectControlsSection
                                    zone_id=zone_id.clone()
                                    layer=layer
                                    effect_cache=effect_cache
                                    on_layers_mutated=on_layers_mutated
                                />
                            </div>
                        }
                    })}
                {is_media
                    .then(|| {
                        view! {
                            <div class="border-t border-edge-subtle/40 pt-3">
                                <MediaPlaybackSection
                                    zone_id=zone_id.clone()
                                    layer=layer
                                    fence=fence
                                    writes=writes
                                />
                            </div>
                        }
                    })}

                // ── Transform & color disclosure ──────────────────────
                <details
                    class="rounded-lg border border-edge-subtle/55 bg-surface-overlay/25"
                    open=expanded
                    on:toggle=move |event| {
                        let element: web_sys::Element = event_target(&event);
                        // Removal can dispatch a final toggle; it must not
                        // overwrite the preference adopted by the new row.
                        if element.is_connected() {
                            on_disclosure.run(element.has_attribute("open"));
                        }
                    }
                >
                    <summary class="flex cursor-pointer items-center gap-1.5 px-3 py-2 text-[11px] font-semibold text-fg-secondary">
                        <Icon icon=LuChevronRight width="12px" height="12px" />
                        "Transform & Color"
                    </summary>
                    <div class="space-y-3 border-t border-edge-subtle/45 px-3 py-3">
                        <SilkSelect
                            aria_label="Fit"
                            value=Signal::derive(move || fit_value(layer.get().transform.fit).to_owned())
                            options=Signal::derive(fit_options)
                            on_change=Callback::new({
                                let zone_id = zone_id.clone();
                                move |value: String| {
                                    let mut next = layer.get_untracked();
                                    next.transform.fit = parse_fit(&value);
                                    update_layer(
                                        zone_id.clone(),
                                        next,
                                        fence.get_untracked(),
                                        writes,
                                    );
                                }
                            })
                            placeholder="Fit"
                            class="border border-edge-subtle bg-surface-sunken/55 px-2.5 py-1.5 text-[11px] text-fg-primary"
                            label_class="font-mono"
                        />
                        <LayerSlider
                            label="Brightness"
                            value=Signal::derive(move || layer.get().adjust.brightness)
                            min=0.0
                            max=4.0
                            step=0.05
                            on_change=Callback::new({
                                let zone_id = zone_id.clone();
                                move |value: f32| {
                                    let mut next = layer.get_untracked();
                                    next.adjust.brightness = value.clamp(0.0, 4.0);
                                    update_layer(
                                        zone_id.clone(),
                                        next,
                                        fence.get_untracked(),
                                        writes,
                                    );
                                }
                            })
                        />
                        <LayerSlider
                            label="Saturation"
                            value=Signal::derive(move || layer.get().adjust.saturation)
                            min=0.0
                            max=4.0
                            step=0.05
                            on_change=Callback::new({
                                let zone_id = zone_id.clone();
                                move |value: f32| {
                                    let mut next = layer.get_untracked();
                                    next.adjust.saturation = value.clamp(0.0, 4.0);
                                    update_layer(
                                        zone_id.clone(),
                                        next,
                                        fence.get_untracked(),
                                        writes,
                                    );
                                }
                            })
                        />
                        <LayerSlider
                            label="Tint"
                            value=Signal::derive(move || layer.get().adjust.tint_strength)
                            min=0.0
                            max=1.0
                            step=0.01
                            on_change=Callback::new({
                                let zone_id = zone_id.clone();
                                move |value: f32| {
                                    let mut next = layer.get_untracked();
                                    next.adjust.tint_strength = value.clamp(0.0, 1.0);
                                    update_layer(
                                        zone_id.clone(),
                                        next,
                                        fence.get_untracked(),
                                        writes,
                                    );
                                }
                            })
                        />
                        <LayerSlider
                            label="Scale X"
                            value=Signal::derive(move || layer.get().transform.scale[0])
                            min=0.1
                            max=4.0
                            step=0.05
                            on_change=Callback::new({
                                let zone_id = zone_id.clone();
                                move |value: f32| {
                                    let mut next = layer.get_untracked();
                                    next.transform.scale[0] = value.clamp(0.1, 4.0);
                                    update_layer(
                                        zone_id.clone(),
                                        next,
                                        fence.get_untracked(),
                                        writes,
                                    );
                                }
                            })
                        />
                        <LayerSlider
                            label="Scale Y"
                            value=Signal::derive(move || layer.get().transform.scale[1])
                            min=0.1
                            max=4.0
                            step=0.05
                            on_change=Callback::new({
                                let zone_id = zone_id.clone();
                                move |value: f32| {
                                    let mut next = layer.get_untracked();
                                    next.transform.scale[1] = value.clamp(0.1, 4.0);
                                    update_layer(
                                        zone_id.clone(),
                                        next,
                                        fence.get_untracked(),
                                        writes,
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
    value: Signal<f32>,
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
                prop:value=move || format!("{:.2}", value.get())
                on:change=move |event| {
                    if let Some(value) = Change::from_event(event).value::<f32>() {
                        on_change.run(value);
                    }
                }
            />
            <span class="text-right tabular-nums">{move || format!("{:.2}", value.get())}</span>
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
