use std::collections::HashMap;

use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::ControlDefinition;
use hypercolor_types::layer::{BlendMode, LayerAdjust, LayerSource, LayerTransform};
use hypercolor_types::scene::ZoneRole;
use leptos::prelude::*;

use crate::api;
use crate::components::layer_panel::source::remembered_effect_layer_source;
use crate::preferences::EffectPreferences;
use crate::toasts;
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

pub(super) async fn apply_effect_to_current_led_zones(ctx: &EffectsContext, effect_id: String) {
    // Bake remembered preferences into the layer source, mirroring the
    // primary-zone apply path — an all-zones apply should start every
    // zone in the user's saved state, not at defaults.
    let prefs = ctx.preferences.get(&effect_id);
    let Ok(source) = remembered_effect_layer_source(&effect_id, prefs.as_ref()) else {
        toasts::toast_error("That effect has an invalid identifier");
        return;
    };
    let scene = match api::fetch_active_scene().await {
        Ok(scene) => scene,
        Err(error) => {
            toasts::toast_error(&format!("Couldn't load the active scene: {error}"));
            return;
        }
    };
    let zone_ids = scene
        .zones
        .iter()
        .filter(|zone| zone.role != ZoneRole::Display)
        .map(|zone| zone.id.to_string())
        .collect::<Vec<_>>();
    if zone_ids.is_empty() {
        toasts::toast_error("No light zones are available");
        return;
    }

    let mut applied = 0_usize;
    let mut failed = 0_usize;
    for zone_id in &zone_ids {
        match apply_effect_layer(zone_id, &source).await {
            Ok(()) => applied += 1,
            Err(_) => failed += 1,
        }
    }
    ctx.refresh_active_scene();
    if failed == 0 {
        toasts::toast_success(&format!("Effect applied to {applied} zone(s)"));
    } else if applied == 0 {
        toasts::toast_error("Effect apply failed");
    } else {
        toasts::toast_error(&format!(
            "Effect applied to {applied} zone(s), {failed} failed"
        ));
    }
}

async fn apply_effect_layer(zone_id: &str, source: &LayerSource) -> api::ApiResult<()> {
    let stack = api::list_layers(zone_id).await?;
    let outcome = if let Some(layer) = stack
        .items
        .iter()
        .find(|layer| matches!(layer.source, LayerSource::Effect { .. }))
    {
        let mut request = api::update_request_from_layer(layer);
        request.source = source.clone();
        api::update_layer(
            zone_id,
            &layer.id.to_string(),
            &request,
            Some(stack.revision),
        )
        .await?
    } else {
        let request = api::CreateLayerRequest {
            name: None,
            source: source.clone(),
            blend: Some(BlendMode::Alpha),
            opacity: Some(1.0),
            transform: Some(LayerTransform::default()),
            adjust: Some(LayerAdjust::default()),
            bindings: None,
            enabled: None,
        };
        api::create_layer(zone_id, &request, Some(stack.revision)).await?
    };
    match outcome {
        api::LayerStackOutcome::Applied(_) => Ok(()),
        api::LayerStackOutcome::Stale { current } => Err(api::ApiError::precondition_failed(
            format!("Layer stack changed at revision {current}"),
        )),
    }
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
