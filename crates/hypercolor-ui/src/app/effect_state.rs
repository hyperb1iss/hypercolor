use std::collections::HashMap;

use hypercolor_types::effect::{ControlDefinition, ControlValue, EffectId};
use hypercolor_types::layer::{LayerAdjust, LayerBlendMode, LayerSource, LayerTransform};
use hypercolor_types::scene::ZoneRole;
use leptos::prelude::*;

use crate::api;
use crate::control_value_json::controls_to_json;
use crate::preferences::EffectPreferences;
use crate::toasts;
use crate::ws::EffectErrorHint;

use super::EffectsContext;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ActiveEffectSnapshot {
    id: Option<String>,
    name: Option<String>,
    category: String,
    controls: Vec<ControlDefinition>,
    control_values: HashMap<String, ControlValue>,
    preset_id: Option<String>,
}

pub(super) async fn apply_effect_to_current_led_zones(ctx: &EffectsContext, effect_id: String) {
    let Some(source) = effect_layer_source(&effect_id) else {
        toasts::toast_error("That effect has an invalid identifier");
        return;
    };
    let scene = match api::fetch_active_scene().await {
        Ok(Some(scene)) => scene,
        _ => {
            toasts::toast_error("No active scene is available");
            return;
        }
    };
    let zone_ids = scene
        .groups
        .iter()
        .filter(|group| group.role != ZoneRole::Display)
        .map(|group| group.id.to_string())
        .collect::<Vec<_>>();
    if zone_ids.is_empty() {
        toasts::toast_error("No light zones are available");
        return;
    }

    let mut applied = 0_usize;
    let mut failed = 0_usize;
    for zone_id in &zone_ids {
        match apply_effect_layer(&scene.id, zone_id, &source).await {
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

async fn apply_effect_layer(
    scene_id: &str,
    zone_id: &str,
    source: &LayerSource,
) -> Result<(), String> {
    let stack = api::list_layers(scene_id, zone_id).await?;
    let outcome = if let Some(layer) = stack
        .items
        .iter()
        .find(|layer| matches!(layer.source, LayerSource::Effect { .. }))
    {
        let mut request = api::UpdateLayerRequest::from(layer);
        request.source = source.clone();
        api::update_layer(
            scene_id,
            zone_id,
            &layer.id.to_string(),
            &request,
            Some(stack.layers_version),
        )
        .await?
    } else {
        let request = api::CreateLayerRequest {
            name: None,
            source: source.clone(),
            blend: LayerBlendMode::Alpha,
            opacity: 1.0,
            transform: LayerTransform::default(),
            adjust: LayerAdjust::default(),
            bindings: Vec::new(),
            enabled: true,
        };
        api::create_layer(scene_id, zone_id, &request, Some(stack.layers_version)).await?
    };
    match outcome {
        api::LayerStackOutcome::Applied(_) => Ok(()),
        api::LayerStackOutcome::Stale { .. } => Err("layer stack changed".to_owned()),
    }
}

fn effect_layer_source(effect_id: &str) -> Option<LayerSource> {
    let uuid = uuid::Uuid::parse_str(effect_id.trim()).ok()?;
    Some(LayerSource::Effect {
        effect_id: EffectId::new(uuid),
        controls: HashMap::new(),
        control_bindings: HashMap::new(),
        preset_id: None,
    })
}

pub(super) fn apply_active_effect_snapshot(
    ctx: &EffectsContext,
    id: String,
    name: String,
    controls: Vec<ControlDefinition>,
    control_values: HashMap<String, ControlValue>,
    active_preset_id: Option<String>,
) {
    let category = ctx
        .effect_summary(&id)
        .map(|effect| effect.category)
        .unwrap_or_default();

    ctx.set_active_effect_name.set(Some(name));
    ctx.set_active_effect_category.set(category);
    ctx.set_active_controls.set(controls);
    ctx.set_active_control_values.set(control_values.clone());
    ctx.set_active_preset_id.set(active_preset_id.clone());
    ctx.set_is_playing.set(true);
    if ctx.active_effect_id.get_untracked().as_deref() != Some(id.as_str()) {
        ctx.set_active_effect_id.set(Some(id.clone()));
    }

    // ── Per-effect preferences: restore or save ───────────────────────
    //
    // Two paths:
    //
    //   1. First snapshot after a switch → RESTORE. The daemon has just
    //      loaded defaults; if our stored preferences differ, re-apply
    //      the saved state to the daemon.
    //
    //   2. Any follow-up snapshot (user picked a preset, tweaked a
    //      control, etc.) → SAVE. The daemon is already in the state
    //      the user just asked for; we just need to capture it.
    //
    // The `restored_effects` set gates this. It's cleared for an effect
    // ID when `apply_effect(id)` is called, so we re-check on each
    // genuine switch, and marked after the first check so subsequent
    // refreshes for the same effect fall through to save.
    let store = ctx.preferences;
    let already_checked = ctx
        .restored_effects
        .with_value(|set| set.contains(id.as_str()));
    if !already_checked {
        ctx.restored_effects.update_value(|set| {
            set.insert(id.clone());
        });

        if let Some(prefs) = store.get(&id) {
            // Compare through the same lossy JSON serializer we use to
            // send controls to the daemon — colors hex-encode to 256
            // steps, so a naive `HashMap` equality would mis-fire
            // thanks to float precision drift on round-trip.
            let stored_json = controls_to_json(&prefs.control_values);
            let daemon_json = controls_to_json(&control_values);
            let needs_restore = prefs.preset_id != active_preset_id || stored_json != daemon_json;
            if needs_restore {
                restore_effect_preferences(*ctx, id, prefs);
                return;
            }
        }
    }

    // Save path — either this was the first snapshot with nothing to
    // restore, or it's a follow-up after user modification. In both
    // cases, capture whatever the daemon just confirmed so switching
    // away and coming back lands us in the same place.
    store.save(
        id,
        EffectPreferences {
            preset_id: active_preset_id,
            control_values,
        },
    );
}

/// Re-applies a remembered preset + control snapshot on top of the
/// daemon's defaults. Fully async: apply_preset first (if any), then
/// update_controls using the same hex-color-encoding serializer the
/// preset picker uses (the daemon silently ignores `ControlValue`'s
/// default tagged JSON), then a final refresh so the UI mirrors the
/// restored daemon state. Bails at every step if the user has switched
/// effects in the meantime; a late-landing restore from effect A would
/// otherwise trample a freshly-activated effect B.
fn restore_effect_preferences(ctx: EffectsContext, effect_id: String, prefs: EffectPreferences) {
    leptos::task::spawn_local(async move {
        if ctx.active_effect_id.get_untracked().as_deref() != Some(effect_id.as_str()) {
            return;
        }

        if let Some(preset_id) = prefs.preset_id.as_ref() {
            let _ = api::apply_preset(preset_id).await;
            if ctx.active_effect_id.get_untracked().as_deref() != Some(effect_id.as_str()) {
                return;
            }
        }

        if !prefs.control_values.is_empty() {
            let controls_json = serde_json::Value::Object(controls_to_json(&prefs.control_values));
            let _ = api::update_controls(&controls_json).await;
            if ctx.active_effect_id.get_untracked().as_deref() != Some(effect_id.as_str()) {
                return;
            }
        }

        // Surface the restored daemon state in the UI. This re-enters
        // `apply_active_effect_snapshot`, but with the effect already
        // present in `restored_effects` so the save branch fires.
        ctx.refresh_active_effect();
    });
}

pub(super) fn clear_active_effect_state(ctx: &EffectsContext) {
    ctx.set_active_effect_id.set(None);
    ctx.set_active_effect_name.set(None);
    ctx.set_active_controls.set(Vec::new());
    ctx.set_active_control_values.set(HashMap::new());
    ctx.set_active_effect_category.set(String::new());
    ctx.set_active_preset_id.set(None);
    ctx.set_is_playing.set(false);
}

pub(super) fn apply_active_scene_snapshot(
    ctx: &EffectsContext,
    active_scene: api::ActiveSceneResponse,
) {
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
        Some("clear_groups") => {
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
            ctx.set_active_effect_name.set(snapshot.name);
            ctx.set_active_effect_category.set(snapshot.category);
            ctx.set_active_controls.set(snapshot.controls);
            ctx.set_active_control_values.set(snapshot.control_values);
            ctx.set_active_preset_id.set(snapshot.preset_id);
        }
        None => clear_active_effect_state(ctx),
    }
}
