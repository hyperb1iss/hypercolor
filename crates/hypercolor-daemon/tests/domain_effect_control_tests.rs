//! Service-level tests for effect application, stop, and invalidation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use hypercolor_core::effect::EffectEntry;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory, EffectId,
    EffectMetadata, EffectSource, EffectState,
};
use hypercolor_types::event::HypercolorEvent;
use hypercolor_types::scene::{
    ColorInterpolation, EasingFunction, Scene, SceneId, SceneKind, SceneMutationMode,
    ScenePriority, SceneScope, TransitionSpec, UnassignedBehavior, ZoneId,
};
use uuid::Uuid;

use hypercolor_daemon::api::AppState;
use hypercolor_daemon::domain::effect::{
    ApplyEffect, RequestedTransition, apply_effect, invalidate_active_zones, stop_effect,
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
        layout_id: None,
        activation_brightness: None,
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
            expected_revision: None,
            transition: RequestedTransition::cut(),
            wake_output: true,
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
