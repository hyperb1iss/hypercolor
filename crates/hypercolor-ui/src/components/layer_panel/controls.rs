//! Rich per-layer detail sections for the layer inspector.
//!
//! An Effect layer carries the effect's own parameter controls; a Media
//! layer carries playback settings. These sections surface them inline so
//! a layer can be tuned without leaving Studio. Effect-control edits go
//! through the dedicated `patch_layer_controls` route — a partial,
//! debounced patch that never restructures the stack — so dragging a
//! slider does not tear the row down between frames.

use hypercolor_leptos_ext::events::Change;
use hypercolor_types::layer::{LayerSource, LoopMode, SceneLayer};
use leptos::prelude::*;
use leptos_use::use_debounce_fn;

use crate::api;
use crate::components::control_panel::ControlPanel;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::control_value_json::apply_control_edit;
use crate::toasts;

use super::update_layer;

/// Accent used for an effect layer's control groups — Studio's layer cyan.
const LAYER_ACCENT_RGB: &str = "128, 255, 234";

/// The effect-parameter controls for an Effect-source layer. Fetches the
/// effect's control schema and renders the shared [`ControlPanel`]; edits
/// are coalesced and patched onto the layer's stored controls.
#[component]
pub fn EffectControlsSection(
    scene_id: String,
    group_id: String,
    layer: SceneLayer,
    layers_version: u64,
) -> impl IntoView {
    let LayerSource::Effect {
        effect_id,
        controls,
        ..
    } = layer.source.clone()
    else {
        return ().into_any();
    };

    let effect_id_str = effect_id.to_string();
    let detail = LocalResource::new({
        let effect_id_str = effect_id_str.clone();
        move || {
            let effect_id_str = effect_id_str.clone();
            async move { api::fetch_effect_detail(&effect_id_str).await }
        }
    });
    let defs = Signal::derive(move || {
        detail
            .get()
            .and_then(Result::ok)
            .map(|detail| detail.controls)
            .unwrap_or_default()
    });

    // Optimistic local control values, and the live layers_version that
    // each patch carries and refreshes — so consecutive edits never go
    // stale against the daemon.
    let values = RwSignal::new(controls);
    let version = RwSignal::new(layers_version);
    let pending = StoredValue::new(serde_json::Map::new());
    let layer_id = layer.id.to_string();

    let flush = {
        let scene_id = scene_id.clone();
        let group_id = group_id.clone();
        let layer_id = layer_id.clone();
        use_debounce_fn(
            move || {
                let batch = pending
                    .try_update_value(std::mem::take)
                    .unwrap_or_default();
                if batch.is_empty() {
                    return;
                }
                let payload = serde_json::Value::Object(batch);
                let scene_id = scene_id.clone();
                let group_id = group_id.clone();
                let layer_id = layer_id.clone();
                leptos::task::spawn_local(async move {
                    match api::patch_layer_controls(
                        &scene_id,
                        &group_id,
                        &layer_id,
                        &payload,
                        Some(version.get_untracked()),
                    )
                    .await
                    {
                        Ok(api::LayerStackOutcome::Applied(stack)) => {
                            version.set(stack.layers_version);
                        }
                        Ok(api::LayerStackOutcome::Stale { current }) => {
                            version.set(current);
                        }
                        Err(error) => {
                            toasts::toast_error(&format!("Effect controls failed: {error}"));
                        }
                    }
                });
            },
            120.0,
        )
    };

    let on_change = Callback::new(move |(name, raw): (String, serde_json::Value)| {
        values.update(|current| {
            *current =
                apply_control_edit(std::mem::take(current), &name, &defs.get_untracked(), &raw);
        });
        pending.update_value(|batch| {
            batch.insert(name, raw);
        });
        flush();
    });

    view! {
        <div class="space-y-2">
            <span class=label_class(LabelSize::Micro, LabelTone::Default)>"Effect controls"</span>
            {move || {
                if detail.get().is_none() {
                    view! {
                        <div class="rounded-lg border border-edge-subtle/50 bg-surface-sunken/40 px-3 py-4 text-center text-[11px] text-fg-tertiary/55">
                            "Loading controls…"
                        </div>
                    }
                        .into_any()
                } else {
                    view! {
                        <ControlPanel
                            controls=defs
                            control_values=Signal::derive(move || values.get())
                            accent_rgb=Signal::derive(|| LAYER_ACCENT_RGB.to_owned())
                            on_change=on_change
                        />
                    }
                        .into_any()
                }
            }}
        </div>
    }
    .into_any()
}

/// Playback settings for a Media-source layer: play speed, loop mode, and
/// auto-play. Each edit rewrites the layer's media playback through the
/// standard layer update.
#[component]
pub fn MediaPlaybackSection(
    scene_id: String,
    group_id: String,
    layer: SceneLayer,
    layers_version: u64,
    on_layers_mutated: Callback<()>,
) -> impl IntoView {
    let LayerSource::Media { playback, .. } = layer.source.clone() else {
        return ().into_any();
    };
    let speed = playback.speed;
    let loop_mode = playback.loop_mode;
    let auto_play = playback.auto_play;

    // Rebuild the layer with a mutated `MediaPlayback` and push it.
    let push = {
        let layer = layer.clone();
        move |mutate: &dyn Fn(&mut hypercolor_types::layer::MediaPlayback)| {
            let mut next = layer.clone();
            if let LayerSource::Media { playback, .. } = &mut next.source {
                mutate(playback);
            }
            update_layer(
                scene_id.clone(),
                group_id.clone(),
                next,
                layers_version,
                on_layers_mutated,
            );
        }
    };
    let push_speed = push.clone();
    let push_auto = push.clone();

    let loop_options = vec![
        ("loop".to_owned(), "Loop".to_owned()),
        ("ping_pong".to_owned(), "Ping-pong".to_owned()),
        ("none".to_owned(), "Play once".to_owned()),
    ];
    let loop_value = match loop_mode {
        LoopMode::Loop => "loop",
        LoopMode::PingPong => "ping_pong",
        LoopMode::None => "none",
    }
    .to_owned();

    view! {
        <div class="space-y-3">
            <span class=label_class(LabelSize::Micro, LabelTone::Default)>"Playback"</span>
            <label class="grid grid-cols-[64px_1fr_44px] items-center gap-2 text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/75">
                <span>"Speed"</span>
                <input
                    type="range"
                    min="0.1"
                    max="4"
                    step="0.05"
                    class="w-full accent-accent"
                    prop:value=format!("{speed:.2}")
                    on:change=move |event| {
                        if let Some(value) = Change::from_event(event).value::<f32>() {
                            push_speed(&|playback| playback.speed = value.clamp(0.1, 4.0));
                        }
                    }
                />
                <span class="text-right tabular-nums">{format!("{speed:.2}×")}</span>
            </label>
            <crate::components::silk_select::SilkSelect
                value=Signal::derive(move || loop_value.clone())
                options=Signal::derive(move || loop_options.clone())
                on_change=Callback::new(move |value: String| {
                    let mode = match value.as_str() {
                        "ping_pong" => LoopMode::PingPong,
                        "none" => LoopMode::None,
                        _ => LoopMode::Loop,
                    };
                    push(&|playback| playback.loop_mode = mode);
                })
                placeholder="Loop"
                class="border border-edge-subtle bg-surface-sunken/55 px-2.5 py-1.5 text-[11px] text-fg-primary"
                label_class="font-mono"
            />
            <button
                type="button"
                class="flex w-full items-center justify-between rounded-lg border border-edge-subtle/60 bg-surface-sunken/40 px-3 py-2 text-[11px] text-fg-secondary chip-interactive"
                on:click=move |_| {
                    push_auto(&|playback| playback.auto_play = !auto_play);
                }
            >
                <span>"Auto-play"</span>
                <LayerToggleTrack on=auto_play />
            </button>
        </div>
    }
    .into_any()
}

/// A compact Luminary toggle track — the switch visual without its own
/// label row, for embedding in a layer card. The card rebuilds on every
/// layer change, so a plain `bool` tracks state without a signal.
#[component]
pub fn LayerToggleTrack(on: bool) -> impl IntoView {
    view! {
        <span
            class="relative inline-block h-4 w-7 shrink-0 rounded-full transition-colors duration-200"
            class=("bg-accent/55", on)
            class=("bg-fg-tertiary/20", !on)
        >
            <span
                class="absolute left-0.5 top-0.5 h-3 w-3 rounded-full bg-white/85 transition-transform duration-200"
                class=("translate-x-3", on)
            />
        </span>
    }
}
