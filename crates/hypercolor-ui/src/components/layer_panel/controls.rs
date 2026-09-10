//! Rich per-layer detail sections for the layer inspector.
//!
//! An Effect layer carries the effect's own parameter controls; a Media
//! layer carries playback settings. These sections surface them inline so
//! a layer can be tuned without leaving Studio. Effect-control edits go
//! through the dedicated `patch_layer_controls` route — a partial,
//! debounced patch that never restructures the stack — so dragging a
//! slider does not tear the row down between frames.

use std::sync::Arc;

use hypercolor_leptos_ext::events::Change;
use hypercolor_types::layer::{LayerSource, LoopMode, SceneLayer};
use leptos::prelude::*;

use crate::api;
use crate::components::control_panel::ControlPanel;
use crate::components::control_panel::capture_group::CaptureSharedControls;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::control_session::{
    ControlPatchConfig, ControlPatchFn, ControlPatchFuture, use_control_patch_session,
};
use crate::toasts;

use super::update_layer;

/// Accent RGB for an effect layer's control groups — the app accent.
const LAYER_ACCENT_RGB: &str = "225, 53, 255";

/// Flush debounce for layer control edits — the Studio inspector's
/// product-contract cadence.
const LAYER_CONTROLS_DEBOUNCE_MS: f64 = 120.0;

/// The effect-parameter controls for an Effect-source layer. Fetches the
/// effect's control schema and renders the shared [`ControlPanel`]; edits
/// are coalesced and patched onto the layer's stored controls.
#[component]
pub fn EffectControlsSection(
    zone_id: String,
    layer: Signal<SceneLayer>,
    effect_cache: super::LayerEffectCache,
    on_layers_mutated: Callback<()>,
) -> impl IntoView {
    let LayerSource::Effect {
        effect_id,
        controls,
        ..
    } = layer.get_untracked().source
    else {
        return ().into_any();
    };

    let effect_id_str = effect_id.to_string();
    let ws = expect_context::<crate::app::WsContext>();
    let detail = api::daemon_resource({
        let effect_id_str = effect_id_str.clone();
        move || {
            let effect_id_str = effect_id_str.clone();
            let generation = ws.connection_generation.get();
            async move {
                if let Some((epoch, detail)) =
                    effect_cache.with_value(|cache| cache.get(&effect_id_str).cloned())
                    && epoch == generation
                {
                    return Ok(detail);
                }
                let detail = api::fetch_effect_detail(&effect_id_str).await?;
                effect_cache.try_update_value(|cache| {
                    cache.insert(effect_id_str, (generation, detail.clone()));
                });
                Ok::<_, api::ApiError>(detail)
            }
        }
    });
    // A same-effect layer replacement retires its write session, but its
    // controls need not collapse to a loading placeholder while it mounts.
    let detail_value = Signal::derive(move || {
        detail.get().and_then(Result::ok).or_else(|| {
            effect_cache
                .with_value(|cache| cache.get(&effect_id_str).map(|(_, detail)| detail.clone()))
        })
    });
    let defs = Signal::derive(move || {
        detail_value
            .get()
            .map(|detail| detail.controls)
            .unwrap_or_default()
    });
    let screen_reactive = Signal::derive(move || {
        detail_value.get().is_some_and(|detail| {
            detail
                .tags
                .iter()
                .any(|tag| tag.eq_ignore_ascii_case("screen-reactive"))
        })
    });

    // Optimistic local control values. Layer identity fences stale patches,
    // so the canonical control route carries no revision token.
    let (values, set_values) = signal(controls);
    let layer_id = layer.get_untracked().id.to_string();

    let session_target = Signal::stored(Some(format!("{zone_id}:{layer_id}")));
    let patch: ControlPatchFn = Arc::new({
        let zone_id = zone_id.clone();
        move |_target: String,
              payload: crate::optimistic_controls::ControlValueMap,
              _version: Option<u64>|
              -> ControlPatchFuture {
            let zone_id = zone_id.clone();
            let layer_id = layer_id.clone();
            Box::pin(async move {
                api::patch_layer_controls(&zone_id, &layer_id, &payload).await?;
                Ok(api::MutationOutcome::Applied(None))
            })
        }
    });
    let session = use_control_patch_session(ControlPatchConfig {
        target: session_target,
        defs,
        set_values,
        initial_version: None,
        debounce_ms: LAYER_CONTROLS_DEBOUNCE_MS,
        patch,
        on_error: Callback::new(|error: String| {
            toasts::toast_error(&format!("Effect controls failed: {error}"));
        }),
        recover: Callback::new(move |()| {
            // A rejected edit can leave the server snapshot unchanged, so
            // memo equality will not trigger the normal reconciliation effect.
            if let LayerSource::Effect { controls, .. } = layer.get_untracked().source {
                set_values.set(controls);
            }
            on_layers_mutated.run(());
        }),
        on_committed: None,
        flush_guard: None,
    });
    Effect::new(move |_| {
        if let LayerSource::Effect { controls, .. } = layer.get().source {
            session.reconcile_values.run(controls);
        }
    });
    let on_change = session.on_change;

    view! {
        <div class="space-y-2">
            <span class=label_class(LabelSize::Micro, LabelTone::Default)>"Effect controls"</span>
            <Show
                when=move || detail_value.get().is_some()
                fallback=move || view! {
                    <div class="rounded-lg border border-edge-subtle/50 bg-surface-sunken/40 px-3 py-4 text-center text-[11px] text-fg-tertiary/55">"Loading controls…"</div>
                }
            >
                <ControlPanel
                    controls=defs
                    control_values=values
                    accent_rgb=Signal::derive(|| LAYER_ACCENT_RGB.to_owned())
                    on_change=on_change
                />
                <CaptureSharedControls
                    visible=screen_reactive
                    accent_rgb=Signal::derive(|| LAYER_ACCENT_RGB.to_owned())
                />
            </Show>
        </div>
    }
    .into_any()
}

/// Playback settings for a Media-source layer: play speed, loop mode, and
/// auto-play. Each edit rewrites the layer's media playback through the
/// standard layer update.
#[component]
pub fn MediaPlaybackSection(
    zone_id: String,
    layer: Signal<SceneLayer>,
    revision: Signal<u64>,
    on_layers_mutated: Callback<()>,
) -> impl IntoView {
    let LayerSource::Media { playback, .. } = layer.get_untracked().source else {
        return ().into_any();
    };
    let playback = Memo::new(move |_| match layer.get().source {
        LayerSource::Media { playback, .. } => playback,
        _ => playback.clone(),
    });

    // Rebuild the layer with a mutated `MediaPlayback` and push it.
    let push = {
        move |mutate: &dyn Fn(&mut hypercolor_types::layer::MediaPlayback)| {
            let mut next = layer.get_untracked();
            if let LayerSource::Media { playback, .. } = &mut next.source {
                mutate(playback);
            }
            update_layer(
                zone_id.clone(),
                next,
                revision.get_untracked(),
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
    let loop_value = move || {
        match playback.get().loop_mode {
            LoopMode::Loop => "loop",
            LoopMode::PingPong => "ping_pong",
            LoopMode::None => "none",
        }
        .to_owned()
    };

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
                    prop:value=move || format!("{:.2}", playback.get().speed)
                    on:change=move |event| {
                        if let Some(value) = Change::from_event(event).value::<f32>() {
                            push_speed(&|playback| playback.speed = value.clamp(0.1, 4.0));
                        }
                    }
                />
                <span class="text-right tabular-nums">{move || format!("{:.2}×", playback.get().speed)}</span>
            </label>
            <crate::components::silk_select::SilkSelect
                value=Signal::derive(loop_value)
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
                    push_auto(&|playback| playback.auto_play = !playback.auto_play);
                }
            >
                <span>"Auto-play"</span>
                <LayerToggleTrack on=Signal::derive(move || playback.get().auto_play) />
            </button>
        </div>
    }
    .into_any()
}

/// A compact toggle track that follows the current layer playback state.
#[component]
pub fn LayerToggleTrack(#[prop(into)] on: Signal<bool>) -> impl IntoView {
    view! {
        <span
            class="relative inline-block h-4 w-7 shrink-0 rounded-full transition-colors duration-200"
            class=("bg-accent/55", move || on.get())
            class=("bg-fg-tertiary/20", move || !on.get())
        >
            <span
                class="absolute left-0.5 top-0.5 h-3 w-3 rounded-full bg-white/85 transition-transform duration-200"
                class=("translate-x-3", move || on.get())
            />
        </span>
    }
}
