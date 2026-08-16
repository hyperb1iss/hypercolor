//! Service-level tests for the effect stop and control transactions
//! (Spec 76 §2.2, §2.3).
//!
//! These four transactions used to be four copies of the same ritual
//! spread across two transports, so what they pin is the shared
//! contract: the snapshot-lock refusal, the `controls_version`
//! precondition, the per-control change events, and the fact that the
//! zone change and its control events publish together in commit order
//! rather than racing each other onto the bus.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use hypercolor_core::effect::EffectEntry;
use hypercolor_types::effect::{
    ControlBinding, ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory,
    EffectId, EffectMetadata, EffectSource, EffectState,
};
use hypercolor_types::event::{HypercolorEvent, ZoneChangeKind};
use hypercolor_types::scene::{
    ColorInterpolation, EasingFunction, Scene, SceneId, SceneKind, SceneMutationMode,
    ScenePriority, SceneScope, TransitionSpec, UnassignedBehavior, ZoneId,
};
use uuid::Uuid;

use hypercolor_daemon::api::AppState;
use hypercolor_daemon::domain::effect::{
    ApplyEffect, ControlsRefusal, RequestedTransition, ResetControls, SetControlBinding,
    UpdateControls, apply_effect, invalidate_active_zones, reset_controls, set_control_binding,
    stop_effect, update_controls,
};
use hypercolor_daemon::domain::{DomainError, MutationContext};

// ── Harness ──────────────────────────────────────────────────────────────

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    (Arc::new(AppState::new_with_data_dir(data_dir)), tempdir)
}

fn slider(id: &str, default: f32) -> ControlDefinition {
    ControlDefinition {
        id: id.to_owned(),
        name: id.to_owned(),
        kind: ControlKind::default(),
        control_type: ControlType::Slider,
        default_value: ControlValue::Float(default),
        min: Some(0.0),
        max: Some(1.0),
        step: None,
        labels: Vec::new(),
        group: None,
        tooltip: None,
        aspect_lock: None,
        preview_source: None,
        binding: None,
    }
}

fn controllable_effect(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} html effect"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_owned()],
        controls: vec![slider("speed", 0.5), slider("intensity", 0.25)],
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Html {
            path: format!("/tmp/{name}.html").into(),
        },
        license: None,
    }
}

async fn insert_effect(state: &AppState, metadata: &EffectMetadata) {
    let entry = EffectEntry {
        metadata: metadata.clone(),
        source_path: format!("/tmp/{}.html", metadata.name).into(),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    };
    let _ = state.effect_registry.write().await.register(entry);
}

fn named_scene(name: &str) -> Scene {
    Scene {
        id: SceneId::new(),
        name: name.to_owned(),
        description: None,
        scope: SceneScope::Full,
        zone_assignments: Vec::new(),
        groups: Vec::new(),
        groups_revision: 0,
        transition: TransitionSpec {
            duration_ms: 1000,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: true,
        metadata: HashMap::new(),
        unassigned_behavior: UnassignedBehavior::Off,
        kind: SceneKind::Named,
        mutation_mode: SceneMutationMode::Live,
    }
}

/// Load `metadata` into the default scene's primary zone and hand back
/// the zone it landed in.
async fn running_effect(state: &AppState, metadata: &EffectMetadata) -> ZoneId {
    insert_effect(state, metadata).await;
    let applied = apply_effect(
        state,
        ApplyEffect {
            effect: metadata.clone(),
            controls: HashMap::new(),
            preset_id: None,
            target_zone: None,
            transition: RequestedTransition::cut(),
        },
        MutationContext::api(),
    )
    .await
    .expect("effect should apply");
    applied.zone.id
}

/// Create a named scene and make it current, so a later snapshot lock
/// applies to a scene the manager actually stores. The Default scene is
/// synthesized and cannot be locked.
async fn activate_named_scene(state: &AppState, name: &str) -> SceneId {
    let scene = named_scene(name);
    let scene_id = scene.id;
    let mut manager = state.scene_manager.write().await;
    manager.create(scene).expect("scene should be created");
    manager
        .activate(&scene_id, None)
        .expect("scene should activate");
    scene_id
}

async fn snapshot_lock(state: &AppState, scene_id: SceneId) {
    let mut manager = state.scene_manager.write().await;
    let mut scene = manager
        .get(&scene_id)
        .cloned()
        .expect("the scene should exist");
    scene.mutation_mode = SceneMutationMode::Snapshot;
    manager.update(scene).expect("scene should update");
}

fn drain_events(
    receiver: &mut tokio::sync::broadcast::Receiver<hypercolor_core::bus::TimestampedEvent>,
) -> Vec<HypercolorEvent> {
    let mut seen = Vec::new();
    while let Ok(timestamped) = receiver.try_recv() {
        seen.push(timestamped.event);
    }
    seen
}

fn sensor_binding() -> ControlBinding {
    ControlBinding {
        sensor: "audio.bass".to_owned(),
        sensor_min: 0.0,
        sensor_max: 1.0,
        target_min: 0.0,
        target_max: 1.0,
        deadband: 0.0,
        smoothing: 0.5,
    }
}

fn changed_control_ids(events: &[HypercolorEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            HypercolorEvent::EffectControlChanged { control_id, .. } => Some(control_id.clone()),
            _ => None,
        })
        .collect()
}

// ── stop_effect ──────────────────────────────────────────────────────────

#[tokio::test]
async fn stop_effect_clears_the_primary_zone_and_announces_the_stop() {
    let (state, _tempdir) = isolated_state();
    let metadata = controllable_effect("aurora");
    running_effect(&state, &metadata).await;
    let mut events = state.event_bus.subscribe_all();

    let stopped = stop_effect(&state, MutationContext::api())
        .await
        .expect("stopping should succeed")
        .expect("an effect was running");

    assert_eq!(stopped.effect.id, metadata.id.to_string());
    assert!(stopped.zone.effect_id.is_none());

    let manager = state.scene_manager.read().await;
    let primary = manager
        .active_scene()
        .and_then(Scene::primary_group)
        .expect("the primary zone survives the stop");
    assert!(primary.effect_id.is_none());
    drop(manager);

    let seen = drain_events(&mut events);
    assert!(
        seen.iter()
            .any(|event| matches!(event, HypercolorEvent::EffectStopped { .. })),
        "the stop must be announced: {seen:?}"
    );
}

/// Both transports asked "is anything running" and got three different
/// refusals out of one helper. There is one answer now, and each
/// transport renders it: no active scene, an idle primary zone, and an
/// effect the registry forgot all read as nothing to stop.
#[tokio::test]
async fn stop_effect_reports_nothing_running_rather_than_failing() {
    let (state, _tempdir) = isolated_state();

    assert!(
        stop_effect(&state, MutationContext::mcp())
            .await
            .expect("an idle daemon is not an error")
            .is_none()
    );
}

#[tokio::test]
async fn stop_effect_conflicts_when_the_active_scene_is_snapshot_locked() {
    let (state, _tempdir) = isolated_state();
    let metadata = controllable_effect("aurora");
    let scene_id = activate_named_scene(&state, "studio").await;
    running_effect(&state, &metadata).await;
    snapshot_lock(&state, scene_id).await;

    let error = stop_effect(&state, MutationContext::api())
        .await
        .expect_err("a snapshot scene refuses runtime rewriting");
    assert!(
        matches!(error, DomainError::Conflict { .. }),
        "expected Conflict, got {error:?}"
    );
}

// ── update_controls ──────────────────────────────────────────────────────

#[tokio::test]
async fn update_controls_patches_the_zone_and_reports_the_new_version() {
    let (state, _tempdir) = isolated_state();
    let metadata = controllable_effect("aurora");
    let zone_id = running_effect(&state, &metadata).await;
    let mut events = state.event_bus.subscribe_all();

    let written = update_controls(
        &state,
        UpdateControls {
            zone_id,
            expected_effect_id: Some(metadata.id),
            effect: metadata.clone(),
            controls: HashMap::from([("speed".to_owned(), ControlValue::Float(0.9))]),
            expected_version: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("the patch should not fail")
    .expect("the zone should accept it");

    assert_eq!(
        written.zone.controls.get("speed"),
        Some(&ControlValue::Float(0.9))
    );
    assert!(written.controls_version > 0);

    let seen = drain_events(&mut events);
    assert_eq!(
        changed_control_ids(&seen),
        vec!["speed".to_owned()],
        "only the control that moved is announced: {seen:?}"
    );
    assert!(
        seen.iter().any(|event| matches!(
            event,
            HypercolorEvent::RenderGroupChanged {
                kind: ZoneChangeKind::ControlsPatched,
                ..
            }
        )),
        "the zone change rides with its control events: {seen:?}"
    );
}

#[tokio::test]
async fn update_controls_refuses_a_stale_controls_version() {
    let (state, _tempdir) = isolated_state();
    let metadata = controllable_effect("aurora");
    let zone_id = running_effect(&state, &metadata).await;

    let first = update_controls(
        &state,
        UpdateControls {
            zone_id,
            expected_effect_id: Some(metadata.id),
            effect: metadata.clone(),
            controls: HashMap::from([("speed".to_owned(), ControlValue::Float(0.9))]),
            expected_version: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("the patch should not fail")
    .expect("the zone should accept it");

    let refusal = update_controls(
        &state,
        UpdateControls {
            zone_id,
            expected_effect_id: Some(metadata.id),
            effect: metadata.clone(),
            controls: HashMap::from([("speed".to_owned(), ControlValue::Float(0.1))]),
            expected_version: Some(first.controls_version.saturating_sub(1)),
        },
        MutationContext::api(),
    )
    .await
    .expect("a stale precondition is a refusal, not a failure")
    .expect_err("the zone should refuse it");

    assert_eq!(
        refusal,
        ControlsRefusal::Stale {
            current: first.controls_version
        }
    );

    let manager = state.scene_manager.read().await;
    let zone = manager
        .active_scene()
        .and_then(Scene::primary_group)
        .expect("the primary zone exists");
    assert_eq!(
        zone.controls.get("speed"),
        Some(&ControlValue::Float(0.9)),
        "the refused patch must not have landed"
    );
}

/// The effect precondition is what keeps a patch aimed at one effect
/// from landing on whatever swapped into the zone behind it.
#[tokio::test]
async fn update_controls_refuses_a_zone_that_swapped_effects() {
    let (state, _tempdir) = isolated_state();
    let first = controllable_effect("aurora");
    let second = controllable_effect("nebula");
    let zone_id = running_effect(&state, &first).await;
    running_effect(&state, &second).await;

    let refusal = update_controls(
        &state,
        UpdateControls {
            zone_id,
            expected_effect_id: Some(first.id),
            effect: first.clone(),
            controls: HashMap::from([("speed".to_owned(), ControlValue::Float(0.9))]),
            expected_version: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("a swapped effect is a refusal, not a failure")
    .expect_err("the zone no longer runs that effect");
    assert_eq!(refusal, ControlsRefusal::ZoneMissing);
}

#[tokio::test]
async fn update_controls_refuses_an_unknown_zone() {
    let (state, _tempdir) = isolated_state();
    let metadata = controllable_effect("aurora");
    running_effect(&state, &metadata).await;

    let refusal = update_controls(
        &state,
        UpdateControls {
            zone_id: ZoneId::new(),
            expected_effect_id: None,
            effect: metadata.clone(),
            controls: HashMap::from([("speed".to_owned(), ControlValue::Float(0.9))]),
            expected_version: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("an unknown zone is a refusal, not a failure")
    .expect_err("there is no such zone");
    assert_eq!(refusal, ControlsRefusal::ZoneMissing);
}

// ── reset_controls ───────────────────────────────────────────────────────

#[tokio::test]
async fn reset_controls_restores_the_metadata_defaults() {
    let (state, _tempdir) = isolated_state();
    let metadata = controllable_effect("aurora");
    let zone_id = running_effect(&state, &metadata).await;
    update_controls(
        &state,
        UpdateControls {
            zone_id,
            expected_effect_id: Some(metadata.id),
            effect: metadata.clone(),
            controls: HashMap::from([
                ("speed".to_owned(), ControlValue::Float(0.9)),
                ("intensity".to_owned(), ControlValue::Float(0.8)),
            ]),
            expected_version: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("the patch should not fail")
    .expect("the zone should accept it");
    let mut events = state.event_bus.subscribe_all();

    let written = reset_controls(
        &state,
        ResetControls {
            zone_id,
            effect: metadata.clone(),
        },
        MutationContext::api(),
    )
    .await
    .expect("the reset should not fail")
    .expect("the zone should accept it");

    assert_eq!(
        written.zone.controls.get("speed"),
        Some(&ControlValue::Float(0.5))
    );
    assert_eq!(
        written.zone.controls.get("intensity"),
        Some(&ControlValue::Float(0.25))
    );

    let seen = drain_events(&mut events);
    let mut changed = changed_control_ids(&seen);
    changed.sort();
    assert_eq!(
        changed,
        vec!["intensity".to_owned(), "speed".to_owned()],
        "every control the reset moved is announced: {seen:?}"
    );
}

#[tokio::test]
async fn reset_controls_conflicts_when_the_active_scene_is_snapshot_locked() {
    let (state, _tempdir) = isolated_state();
    let metadata = controllable_effect("aurora");
    let scene_id = activate_named_scene(&state, "studio").await;
    let zone_id = running_effect(&state, &metadata).await;
    snapshot_lock(&state, scene_id).await;

    let error = reset_controls(
        &state,
        ResetControls {
            zone_id,
            effect: metadata,
        },
        MutationContext::api(),
    )
    .await
    .expect_err("a snapshot scene refuses runtime rewriting");
    assert!(
        matches!(error, DomainError::Conflict { .. }),
        "expected Conflict, got {error:?}"
    );
}

// ── set_control_binding ──────────────────────────────────────────────────

#[tokio::test]
async fn set_control_binding_attaches_the_binding_to_the_zone() {
    let (state, _tempdir) = isolated_state();
    let metadata = controllable_effect("aurora");
    let zone_id = running_effect(&state, &metadata).await;
    let mut events = state.event_bus.subscribe_all();

    let written = set_control_binding(
        &state,
        SetControlBinding {
            zone_id,
            control_id: "speed".to_owned(),
            binding: sensor_binding(),
        },
        MutationContext::api(),
    )
    .await
    .expect("the binding should not fail")
    .expect("the zone exists");

    let binding = written
        .zone
        .control_bindings
        .get("speed")
        .expect("the zone carries the binding");
    assert_eq!(binding.sensor, "audio.bass");

    let seen = drain_events(&mut events);
    assert!(
        seen.iter().any(|event| matches!(
            event,
            HypercolorEvent::RenderGroupChanged {
                kind: ZoneChangeKind::Updated,
                ..
            }
        )),
        "the zone change must be announced: {seen:?}"
    );
}

#[tokio::test]
async fn set_control_binding_refuses_an_unknown_zone() {
    let (state, _tempdir) = isolated_state();
    let metadata = controllable_effect("aurora");
    running_effect(&state, &metadata).await;

    let refusal = set_control_binding(
        &state,
        SetControlBinding {
            zone_id: ZoneId::new(),
            control_id: "speed".to_owned(),
            binding: sensor_binding(),
        },
        MutationContext::api(),
    )
    .await
    .expect("an unknown zone is a refusal, not a failure")
    .expect_err("there is no such zone");
    assert_eq!(refusal, ControlsRefusal::ZoneMissing);
}

#[tokio::test]
async fn set_control_binding_conflicts_when_the_active_scene_is_snapshot_locked() {
    let (state, _tempdir) = isolated_state();
    let metadata = controllable_effect("aurora");
    let scene_id = activate_named_scene(&state, "studio").await;
    let zone_id = running_effect(&state, &metadata).await;
    snapshot_lock(&state, scene_id).await;

    let error = set_control_binding(
        &state,
        SetControlBinding {
            zone_id,
            control_id: "speed".to_owned(),
            binding: sensor_binding(),
        },
        MutationContext::api(),
    )
    .await
    .expect_err("a snapshot scene refuses runtime rewriting");
    assert!(
        matches!(error, DomainError::Conflict { .. }),
        "expected Conflict, got {error:?}"
    );
}

// ── invalidate_active_zones ──────────────────────────────────────────────

/// A dropped invalidation would leave the resolved zones pointing at
/// pre-reload effect metadata, so the reconciliation retries rather than
/// surfacing a conflict nobody is listening for.
#[tokio::test]
async fn invalidating_the_active_zones_advances_the_revision_every_time() {
    let (state, _tempdir) = isolated_state();
    let metadata = controllable_effect("aurora");
    running_effect(&state, &metadata).await;

    let before = {
        let manager = state.scene_manager.read().await;
        manager.active_render_groups_revision()
    };
    invalidate_active_zones(&state)
        .await
        .expect("the invalidation should land");
    let after = {
        let manager = state.scene_manager.read().await;
        manager.active_render_groups_revision()
    };
    assert!(
        after > before,
        "the resolved zones must be recomputed: {before} -> {after}"
    );
}
