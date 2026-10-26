use std::collections::HashMap;

use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::ControlDefinition;
use leptos::prelude::*;

use crate::api;
use crate::preferences::EffectPreferences;
use crate::ws::EffectErrorHint;

use super::EffectsContext;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ActiveEffectSnapshot {
    id: Option<String>,
    target: Option<api::EffectLayerTarget>,
    name: Option<String>,
    category: String,
    controls: Vec<ControlDefinition>,
    control_values: HashMap<String, ControlValue>,
    preset_id: Option<String>,
}

pub(super) fn apply_active_effect_snapshot(ctx: &EffectsContext, active: api::PrimaryEffectView) {
    let api::PrimaryEffectView {
        id,
        zone_id,
        layer_id,
        name,
        controls,
        control_values,
        active_preset_id,
        scene_revision: _,
    } = active;
    let target = api::EffectLayerTarget {
        effect_id: id.clone(),
        zone_id,
        layer_id,
    };
    let category = ctx
        .effect_summary(&id)
        .map(|effect| effect.category.as_str().to_owned())
        .unwrap_or_default();

    ctx.set_active_effect_name.set(Some(name));
    ctx.set_active_effect_category.set(category);
    ctx.set_active_controls.set(controls);
    ctx.set_active_control_values.set(control_values.clone());
    ctx.set_active_preset_id.set(active_preset_id.clone());
    ctx.set_is_playing.set(true);
    if ctx.active_effect_target.get_untracked().as_ref() != Some(&target) {
        ctx.set_active_effect_target.set(Some(target));
    }
    if ctx.active_effect_id.get_untracked().as_deref() != Some(id.as_str()) {
        ctx.set_active_effect_id.set(Some(id.clone()));
    }

    // Capture whatever the daemon confirmed so switching away and coming
    // back lands in the same place. This tab's own applies already carry
    // the remembered preset and controls in their one apply request, so
    // a snapshot never writes back: when another client (the CLI, MCP, a
    // script, another tab) switched the effect, re-applying the saved
    // state here would add a second scene mutation, rebuild the effect's
    // renderer, and overrule what that client asked for.
    if let Err(error) = ctx.preferences.save(
        id,
        EffectPreferences {
            preset_id: active_preset_id,
            control_values,
        },
    ) {
        log::error!("failed to persist active effect preferences: {error}");
    }
}

pub(super) fn clear_active_effect_state(ctx: &EffectsContext) {
    ctx.set_active_effect_id.set(None);
    ctx.set_active_effect_target.set(None);
    ctx.set_active_effect_name.set(None);
    ctx.set_active_controls.set(Vec::new());
    ctx.set_active_control_values.set(HashMap::new());
    ctx.set_active_effect_category.set(String::new());
    ctx.set_active_preset_id.set(None);
    ctx.set_is_playing.set(false);
}

pub(super) fn apply_active_scene_snapshot(ctx: &EffectsContext, active_scene: api::SceneDocument) {
    ctx.set_active_scene_name.set(Some(active_scene.name));
    ctx.set_active_scene_kind.set(Some(active_scene.kind));
    ctx.set_active_scene_mutation_mode
        .set(Some(active_scene.mutation_mode));
}

pub(super) fn clear_active_scene_state(ctx: &EffectsContext) {
    ctx.set_active_scene_name.set(None);
    ctx.set_active_scene_kind.set(None);
    ctx.set_active_scene_mutation_mode.set(None);
}

fn effect_error_display_name(ctx: &EffectsContext, effect_id: &str) -> String {
    ctx.effect_summary(effect_id)
        .map(|effect| effect.name)
        .unwrap_or_else(|| effect_id.to_owned())
}

pub(super) fn effect_error_toast_message(
    ctx: &EffectsContext,
    effect_error: &EffectErrorHint,
) -> String {
    let effect_name = effect_error_display_name(ctx, &effect_error.effect_id);
    match effect_error.fallback.as_deref() {
        Some("clear_zones") => {
            format!("{effect_name} crashed and was cleared from the active scene.")
        }
        Some(fallback) if !fallback.is_empty() => {
            format!("{effect_name} crashed. Fallback: {fallback}.")
        }
        _ => format!("{effect_name} hit a render failure."),
    }
}

pub(super) fn capture_active_effect_state(ctx: &EffectsContext) -> ActiveEffectSnapshot {
    ActiveEffectSnapshot {
        id: ctx.active_effect_id.get_untracked(),
        target: ctx.active_effect_target.get_untracked(),
        name: ctx.active_effect_name.get_untracked(),
        category: ctx.active_effect_category.get_untracked(),
        controls: ctx.active_controls.get_untracked(),
        control_values: ctx.active_control_values.get_untracked(),
        preset_id: ctx.active_preset_id.get_untracked(),
    }
}

pub(super) fn restore_active_effect_state(ctx: &EffectsContext, snapshot: ActiveEffectSnapshot) {
    match snapshot.id {
        Some(id) => {
            ctx.set_active_effect_id.set(Some(id));
            ctx.set_active_effect_target.set(snapshot.target);
            ctx.set_active_effect_name.set(snapshot.name);
            ctx.set_active_effect_category.set(snapshot.category);
            ctx.set_active_controls.set(snapshot.controls);
            ctx.set_active_control_values.set(snapshot.control_values);
            ctx.set_active_preset_id.set(snapshot.preset_id);
        }
        None => clear_active_effect_state(ctx),
    }
}
