use std::collections::HashMap;

use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::{ControlDefinition, EffectId};
use hypercolor_types::layer::{BlendMode, LayerAdjust, LayerSource, LayerTransform};
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
    let Some(source) = effect_layer_source(&effect_id, prefs.as_ref()) else {
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
        api::LayerStackOutcome::Stale { current } => Err(api::ApiError::Http {
            status: 412,
            message: Some(format!("Layer stack changed at revision {current}")),
        }),
    }
}

fn effect_layer_source(effect_id: &str, prefs: Option<&EffectPreferences>) -> Option<LayerSource> {
    let uuid = uuid::Uuid::parse_str(effect_id.trim()).ok()?;
    Some(LayerSource::Effect {
        effect_id: EffectId::new(uuid),
        controls: prefs
            .map(|prefs| prefs.control_values.clone())
            .unwrap_or_default(),
        control_bindings: HashMap::new(),
        preset_id: prefs
            .and_then(|prefs| prefs.preset_id.as_deref())
            .and_then(|raw| uuid::Uuid::parse_str(raw.trim()).ok())
            .map(hypercolor_types::library::PresetId),
    })
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
        scene_revision,
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
        ctx.set_active_effect_target.set(Some(target.clone()));
    }
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
                restore_effect_preferences(*ctx, target, scene_revision, prefs);
                return;
            }
        }
    }

    // Save path — either this was the first snapshot with nothing to
    // restore, or it's a follow-up after user modification. In both
    // cases, capture whatever the daemon just confirmed so switching
    // away and coming back lands us in the same place.
    if let Err(error) = store.save(
        id,
        EffectPreferences {
            preset_id: active_preset_id,
            control_values,
        },
    ) {
        log::error!("failed to persist active effect preferences: {error}");
    }
}

/// Restores a remembered preset and its exact derived control snapshot.
fn restore_effect_preferences(
    ctx: EffectsContext,
    mut target: api::EffectLayerTarget,
    scene_revision: u64,
    prefs: EffectPreferences,
) {
    leptos::task::spawn_local(async move {
        if ctx.active_effect_target.get_untracked().as_ref() != Some(&target) {
            return;
        }
        let effect_id = target.effect_id.clone();

        let resolved_preset_id = prefs
            .preset_id
            .as_deref()
            .filter(|preset_id| uuid::Uuid::parse_str(preset_id).is_ok())
            .map(str::to_owned);
        if let Some(preset_id) = resolved_preset_id.as_ref() {
            match api::apply_effect_preset(
                &effect_id,
                preset_id,
                Some(&target.zone_id),
                scene_revision,
            )
            .await
            {
                Ok(replacement) => {
                    if ctx.active_effect_target.get_untracked().as_ref() != Some(&target) {
                        return;
                    }
                    ctx.set_active_effect_target.set(Some(replacement.clone()));
                    target = replacement;
                }
                Err(error) => {
                    crate::toasts::toast_error(&format!("Couldn't restore preset: {error}"));
                    ctx.refresh_active_effect();
                    return;
                }
            }
        }

        if !prefs.control_values.is_empty() {
            if ctx.active_effect_target.get_untracked().as_ref() != Some(&target) {
                return;
            }
            if let Err(error) = patch_restored_controls(&target, &prefs.control_values).await {
                crate::toasts::toast_error(&format!("Couldn't restore saved controls: {error}"));
            }
            if ctx.active_effect_target.get_untracked().as_ref() != Some(&target) {
                return;
            }
        }

        if prefs.preset_id != resolved_preset_id
            && let Err(error) = ctx.preferences.save(
                effect_id.clone(),
                EffectPreferences {
                    preset_id: resolved_preset_id,
                    control_values: prefs.control_values,
                },
            )
        {
            log::error!("failed to persist repaired effect preferences: {error}");
        }

        // Surface the restored daemon state in the UI. This re-enters
        // `apply_active_effect_snapshot`, but with the effect already
        // present in `restored_effects` so the save branch fires.
        ctx.refresh_active_effect();
    });
}

async fn patch_restored_controls(
    target: &api::EffectLayerTarget,
    controls: &HashMap<String, ControlValue>,
) -> api::ApiResult<()> {
    patch_restored_controls_with(target, controls, |zone_id, layer_id, controls| async move {
        api::patch_layer_controls(&zone_id, &layer_id, &controls).await
    })
    .await
}

async fn patch_restored_controls_with<Patch, PatchFuture>(
    target: &api::EffectLayerTarget,
    controls: &HashMap<String, ControlValue>,
    patch: Patch,
) -> api::ApiResult<()>
where
    Patch: FnOnce(String, String, HashMap<String, ControlValue>) -> PatchFuture,
    PatchFuture: std::future::Future<Output = api::ApiResult<()>>,
{
    patch(
        target.zone_id.clone(),
        target.layer_id.clone(),
        controls.clone(),
    )
    .await
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};

    use hypercolor_types::control::ControlValue;

    use super::patch_restored_controls_with;
    use crate::api::{ApiResult, EffectLayerTarget};

    struct SuspendedPatch {
        current_layer: Rc<RefCell<String>>,
        replacement_layer: String,
        suspended: bool,
    }

    impl Future for SuspendedPatch {
        type Output = ApiResult<()>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if !self.suspended {
                self.suspended = true;
                self.current_layer.replace(self.replacement_layer.clone());
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            Poll::Ready(Ok(()))
        }
    }

    #[test]
    fn suspended_same_effect_restoration_keeps_the_observed_layer_target() {
        let old_layer = "00000000-0000-0000-0000-000000000003";
        let new_layer = "00000000-0000-0000-0000-000000000004";
        let target = EffectLayerTarget {
            effect_id: "00000000-0000-0000-0000-00000000000a".to_owned(),
            zone_id: "00000000-0000-0000-0000-000000000002".to_owned(),
            layer_id: old_layer.to_owned(),
        };
        let current_layer = Rc::new(RefCell::new(old_layer.to_owned()));
        let addressed_layer = Rc::new(RefCell::new(None::<String>));
        let patch_current_layer = Rc::clone(&current_layer);
        let patch_addressed_layer = Rc::clone(&addressed_layer);
        let controls =
            std::collections::HashMap::from([("speed".to_owned(), ControlValue::Float(0.5))]);
        let future = patch_restored_controls_with(&target, &controls, move |_, layer_id, _| {
            patch_addressed_layer.replace(Some(layer_id));
            SuspendedPatch {
                current_layer: patch_current_layer,
                replacement_layer: new_layer.to_owned(),
                suspended: false,
            }
        });
        let mut future = std::pin::pin!(future);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
        assert_eq!(current_layer.borrow().as_str(), new_layer);
        assert_eq!(addressed_layer.borrow().as_deref(), Some(old_layer));
        assert!(matches!(
            future.as_mut().poll(&mut context),
            Poll::Ready(Ok(()))
        ));
        assert_eq!(addressed_layer.borrow().as_deref(), Some(old_layer));
    }
}
