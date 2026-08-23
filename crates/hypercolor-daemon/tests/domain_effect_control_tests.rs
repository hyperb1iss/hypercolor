//! Service-level tests for effect application and invalidation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use hypercolor_core::effect::EffectEntry;
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId, EffectMetadata,
    EffectSource, EffectState,
};
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::ZoneId;
use uuid::Uuid;

use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::domain::MutationContext;
use hypercolor_daemon::domain::effect::{
    ApplyEffect, RequestedTransition, apply_effect, invalidate_active_zones,
};
use hypercolor_daemon::domain::layer::insert_layer;

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
        default_value: ControlValue::Float(f64::from(default)),
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
    let _ = state.domains.effects.register(entry).await;
}

/// Load `metadata` into the default scene's primary zone and hand back
/// the zone it landed in.
async fn running_effect(state: &AppState, metadata: &EffectMetadata) -> ZoneId {
    insert_effect(state, metadata).await;
    let resolved = state
        .domains
        .effects
        .metadata_for_mutation(metadata.id)
        .await
        .expect("registered effect should resolve");
    let applied = apply_effect(
        &state.domains.effects,
        ApplyEffect {
            effect: resolved,
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

#[tokio::test]
async fn invalidating_the_active_zones_advances_the_revision_every_time() {
    let (state, _tempdir) = isolated_state();
    let metadata = controllable_effect("aurora");
    running_effect(&state, &metadata).await;

    let before = {
        let manager = state.scene_manager.snapshot().await;
        manager.resolved_zones_revision()
    };
    invalidate_active_zones(&state.domains.effects)
        .await
        .expect("the invalidation should land");
    let after = {
        let manager = state.scene_manager.snapshot().await;
        manager.resolved_zones_revision()
    };
    assert!(
        after > before,
        "the resolved zones must be recomputed: {before} -> {after}"
    );
}

#[tokio::test]
async fn apply_effect_rejects_unvalidated_controls_inside_the_domain() {
    let (state, _tempdir) = isolated_state();
    let metadata = controllable_effect("aurora");
    insert_effect(&state, &metadata).await;
    let before = state.domains.scene.revision();
    let resolved = state
        .domains
        .effects
        .metadata_for_mutation(metadata.id)
        .await
        .expect("registered effect should resolve");

    let error = apply_effect(
        &state.domains.effects,
        ApplyEffect {
            effect: resolved,
            controls: HashMap::from([("speed".to_owned(), ControlValue::Bool(true))]),
            preset_id: None,
            target_zone: None,
            expected_revision: None,
            transition: RequestedTransition::cut(),
            wake_output: true,
        },
        MutationContext::api(),
    )
    .await
    .expect_err("a direct domain caller cannot bypass effect schema admission");

    assert!(error.to_string().contains("control values were rejected"));
    assert_eq!(state.domains.scene.revision(), before);
}

#[tokio::test]
async fn layer_insertion_normalizes_controls_under_catalog_admission() {
    let (state, _tempdir) = isolated_state();
    let metadata = controllable_effect("aurora");
    let zone_id = running_effect(&state, &metadata).await;
    let before = state.domains.scene.revision();
    let layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        metadata.id,
        HashMap::from([("speed".to_owned(), ControlValue::Bool(true))]),
        HashMap::new(),
        None,
    );

    let error = insert_layer(&state.domains.effects, zone_id, layer, None, None)
        .await
        .expect_err("layer creation cannot publish an invalid effect control");

    assert!(error.to_string().contains("control values were rejected"));
    assert_eq!(state.domains.scene.revision(), before);
}
