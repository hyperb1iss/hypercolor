//! Service-level tests for the scene domain layer (Spec 76 §2.3).
//!
//! These exercise `apply_effect` and `activate_scene` through the
//! service surface rather than through a transport, so what they pin is
//! the contract both REST and MCP now share: the compare-and-swap that
//! guards the short lock scopes, the durability receipt, and the
//! ordering the commit sequencer imposes on publication.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use hypercolor_core::asset::{AssetTypeHint, AssetUploadOptions};
use hypercolor_core::effect::EffectEntry;
use hypercolor_types::asset::AssetId;
use hypercolor_types::effect::{
    EffectCategory, EffectId, EffectMetadata, EffectSource, EffectState,
};
use hypercolor_types::event::{HypercolorEvent, ZoneChangeKind};
use hypercolor_types::layer::{
    LayerAdjust, LayerBlendMode, LayerSource, LayerTransform, MediaPlayback, SceneLayer,
    SceneLayerId,
};
use hypercolor_types::scene::{
    ColorInterpolation, EasingFunction, Scene, SceneId, SceneKind, SceneMutationMode,
    ScenePriority, SceneScope, TransitionSpec, UnassignedBehavior,
};
use uuid::Uuid;

use hypercolor_daemon::api::AppState;
use hypercolor_daemon::domain::commit::CommitDurability;
use hypercolor_daemon::domain::effect::{ApplyEffect, RequestedTransition, apply_effect};
use hypercolor_daemon::domain::scene::{ActivateScene, activate_scene, commit_scene};
use hypercolor_daemon::domain::{DomainError, MutationContext};

// ── Harness ──────────────────────────────────────────────────────────────

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    (Arc::new(AppState::new_with_data_dir(data_dir)), tempdir)
}

fn test_effect_metadata(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} html effect"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_owned()],
        controls: Vec::new(),
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

async fn insert_lottie_asset(state: &AppState, name: &str, seed: u8) -> AssetId {
    let mut options = AssetUploadOptions::new(name);
    options.type_hint = Some(AssetTypeHint::Lottie);
    let upsert = state
        .asset_library
        .write()
        .await
        .add_bytes(
            format!(r#"{{"v":"5.7.4","fr":{seed},"layers":[]}}"#).as_bytes(),
            options,
        )
        .expect("lottie asset should upload");
    upsert.record.id
}

fn media_layer(asset_id: AssetId) -> SceneLayer {
    SceneLayer {
        id: SceneLayerId::new(),
        name: None,
        source: LayerSource::Media {
            asset_id,
            playback: MediaPlayback::default(),
        },
        blend: LayerBlendMode::default(),
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }
}

fn apply_command(effect_id: EffectId) -> ApplyEffect {
    ApplyEffect {
        effect_id,
        controls: HashMap::new(),
        preset_id: None,
        target_zone: None,
        transition: RequestedTransition::cut(),
    }
}

// ── apply_effect ─────────────────────────────────────────────────────────

#[tokio::test]
async fn apply_effect_loads_the_primary_zone_and_commits_durably() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;

    let applied = apply_effect(&state, apply_command(metadata.id), MutationContext::api())
        .await
        .expect("apply should succeed");

    assert_eq!(applied.effect.id, metadata.id.to_string());
    assert_eq!(applied.effect.name, "aurora");
    assert!(applied.previous_effect.is_none());
    assert_eq!(applied.transition.style, "cut");
    assert_eq!(applied.transition.duration_ms, 0);
    assert_eq!(applied.commit.durability(), CommitDurability::Written);
    assert!(applied.commit.retry_error().is_none());

    let manager = state.scene_manager.read().await;
    let primary = manager
        .active_scene()
        .and_then(Scene::primary_group)
        .expect("the active scene should have a primary zone");
    assert_eq!(primary.effect_id, Some(metadata.id));
}

#[tokio::test]
async fn apply_effect_reports_the_outgoing_effect_of_the_target_zone() {
    let (state, _tempdir) = isolated_state();
    let first = test_effect_metadata("aurora");
    let second = test_effect_metadata("nebula");
    insert_effect(&state, &first).await;
    insert_effect(&state, &second).await;

    apply_effect(&state, apply_command(first.id), MutationContext::api())
        .await
        .expect("first apply should succeed");
    let applied = apply_effect(&state, apply_command(second.id), MutationContext::api())
        .await
        .expect("second apply should succeed");

    let previous = applied
        .previous_effect
        .expect("the second apply should report the first effect");
    assert_eq!(previous.id, first.id.to_string());
    assert_eq!(applied.zone_change, ZoneChangeKind::Updated);
}

#[tokio::test]
async fn apply_effect_refuses_an_unknown_effect() {
    let (state, _tempdir) = isolated_state();
    let error = apply_effect(
        &state,
        apply_command(EffectId::new(Uuid::now_v7())),
        MutationContext::api(),
    )
    .await
    .expect_err("an unregistered effect should not apply");
    assert!(
        matches!(error, DomainError::NotFound { .. }),
        "expected NotFound, got {error:?}"
    );
}

#[tokio::test]
async fn apply_effect_refuses_a_display_face() {
    let (state, _tempdir) = isolated_state();
    let mut metadata = test_effect_metadata("clock-face");
    metadata.category = EffectCategory::Display;
    insert_effect(&state, &metadata).await;

    let error = apply_effect(&state, apply_command(metadata.id), MutationContext::api())
        .await
        .expect_err("a display face should not reach the LED pipeline");
    assert!(
        matches!(error, DomainError::Validation { .. }),
        "expected Validation, got {error:?}"
    );
}

#[tokio::test]
async fn apply_effect_refuses_an_unimplemented_transition_from_either_transport() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;

    // This is the divergence the unified surface closes: MCP used to
    // accept a duration here, echo it back, and never apply it.
    for trigger in [MutationContext::api(), MutationContext::mcp()] {
        let mut command = apply_command(metadata.id);
        command.transition = RequestedTransition::of_duration(500);
        let error = apply_effect(&state, command, trigger)
            .await
            .expect_err("a non-zero transition is not implemented");
        match error {
            DomainError::Validation { message, .. } => assert!(
                message.contains("not implemented yet"),
                "unexpected message: {message}"
            ),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    // A zero-duration cut is the one request the daemon can honor.
    let applied = apply_effect(
        &state,
        ApplyEffect {
            transition: RequestedTransition::of_duration(0),
            ..apply_command(metadata.id)
        },
        MutationContext::mcp(),
    )
    .await
    .expect("an immediate cut applies");
    assert_eq!(applied.transition.duration_ms, 0);
}

#[tokio::test]
async fn apply_effect_conflicts_when_the_active_scene_is_snapshot_locked() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;

    let mut scene = named_scene("frozen");
    scene.mutation_mode = SceneMutationMode::Snapshot;
    let scene_id = scene.id;
    {
        let mut manager = state.scene_manager.write().await;
        manager.create(scene).expect("scene should be created");
        manager
            .activate(&scene_id, None)
            .expect("scene should activate");
    }

    let error = apply_effect(&state, apply_command(metadata.id), MutationContext::mcp())
        .await
        .expect_err("a snapshot scene refuses runtime rewriting");
    assert!(
        matches!(error, DomainError::Conflict { .. }),
        "expected Conflict, got {error:?}"
    );
}

// ── activate_scene ───────────────────────────────────────────────────────

#[tokio::test]
async fn activate_scene_switches_the_current_scene_and_publishes_once() {
    let (state, _tempdir) = isolated_state();
    let scene = named_scene("evening");
    let scene_id = scene.id;
    {
        let mut manager = state.scene_manager.write().await;
        manager.create(scene).expect("scene should be created");
    }
    let mut events = state.event_bus.subscribe_all();

    let activated = activate_scene(
        &state,
        ActivateScene {
            scene_id,
            transition: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("activation should succeed");

    assert_eq!(activated.scene_id, scene_id);
    assert_eq!(activated.scene_name, "evening");
    assert_eq!(activated.commit.durability(), CommitDurability::Written);

    let mut active_changed = 0;
    while let Ok(timestamped) = events.try_recv() {
        if let HypercolorEvent::ActiveSceneChanged { current, .. } = timestamped.event {
            assert_eq!(current, scene_id);
            active_changed += 1;
        }
    }
    assert_eq!(active_changed, 1, "exactly one activation announcement");

    let manager = state.scene_manager.read().await;
    assert_eq!(manager.active_scene_id().copied(), Some(scene_id));
}

#[tokio::test]
async fn activate_scene_honors_a_transition_override() {
    let (state, _tempdir) = isolated_state();
    let first = named_scene("evening");
    let second = named_scene("night");
    let (first_id, second_id) = (first.id, second.id);
    {
        let mut manager = state.scene_manager.write().await;
        manager.create(first).expect("scene should be created");
        manager.create(second).expect("scene should be created");
    }

    activate_scene(
        &state,
        ActivateScene {
            scene_id: first_id,
            transition: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("first activation should succeed");

    // MCP passes a duration override; the service applies it rather
    // than echoing it, which is why it is a command field and not an
    // adapter detail.
    activate_scene(
        &state,
        ActivateScene {
            scene_id: second_id,
            transition: Some(TransitionSpec {
                duration_ms: 2_500,
                easing: EasingFunction::Linear,
                color_interpolation: ColorInterpolation::Oklab,
            }),
        },
        MutationContext::mcp(),
    )
    .await
    .expect("second activation should succeed");

    let manager = state.scene_manager.read().await;
    let transition = manager
        .active_transition()
        .expect("a non-zero override should start a transition");
    assert_eq!(transition.spec.duration_ms, 2_500);
}

#[tokio::test]
async fn activate_scene_refuses_an_unknown_scene() {
    let (state, _tempdir) = isolated_state();
    let error = activate_scene(
        &state,
        ActivateScene {
            scene_id: SceneId::new(),
            transition: None,
        },
        MutationContext::api(),
    )
    .await
    .expect_err("an unknown scene should not activate");
    assert!(
        matches!(error, DomainError::NotFound { .. }),
        "expected NotFound, got {error:?}"
    );
}

/// MCP scene activation used to skip the soft admission REST performs,
/// so a media-heavy scene could activate over MCP without the
/// preemptive render downshift that protects frame budget. Driving the
/// MCP tool proves the check now runs on that path too.
#[tokio::test]
async fn mcp_scene_activation_applies_media_soft_admission() {
    let (state, _tempdir) = isolated_state();

    // Eight distinct Lottie producers cost 64ms of frame budget against
    // a 60ms soft cap. Lottie carries no hard cap, so this exercises the
    // soft path without tripping the hard one.
    let mut zone = {
        let spatial = state.spatial_engine.read().await;
        hypercolor_core::scene::default_primary_group(spatial.layout().as_ref().clone())
    };
    for index in 0..8u8 {
        let asset_id = insert_lottie_asset(&state, &format!("sparkle-{index}.json"), index).await;
        zone.layers.push(media_layer(asset_id));
    }

    let mut scene = named_scene("cinema");
    scene.groups = vec![zone];
    let scene_id = scene.id;
    {
        let mut manager = state.scene_manager.write().await;
        manager.create(scene).expect("scene should be created");
    }

    let tier_before = state.render_loop.read().await.stats().tier;
    assert!(
        tier_before.downshift().is_some(),
        "the harness must start above the minimum tier for this to prove anything"
    );

    let result = hypercolor_daemon::mcp::tools::execute_tool_with_state(
        "activate_scene",
        &serde_json::json!({ "name": "cinema" }),
        &state,
    )
    .await
    .expect("activation should succeed");
    assert_eq!(result["activated"], true);

    let tier_after = state.render_loop.read().await.stats().tier;
    assert_eq!(
        Some(tier_after),
        tier_before.downshift(),
        "an over-budget scene downshifts the render tier on activation"
    );

    let manager = state.scene_manager.read().await;
    assert_eq!(manager.active_scene_id().copied(), Some(scene_id));
}

// ── Commit contract ──────────────────────────────────────────────────────

#[tokio::test]
async fn a_stale_base_revision_is_rejected_before_admission() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;
    let layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };

    // Two candidates from the same base revision. The first wins.
    let mut stale = state.begin_scene_mutation().await;
    let mut winner = state.begin_scene_mutation().await;
    assert_eq!(stale.base_revision(), winner.base_revision());

    winner
        .upsert_primary_zone(&metadata, HashMap::new(), None, layout.clone())
        .expect("candidate mutation should apply");
    let commit = commit_scene(&state, winner)
        .await
        .expect("the first commit wins");
    assert_eq!(commit.durability(), CommitDurability::Written);

    stale
        .upsert_primary_zone(&metadata, HashMap::new(), None, layout)
        .expect("candidate mutation should apply");
    let error = commit_scene(&state, stale)
        .await
        .expect_err("a candidate built from a dead revision must not land");
    match error {
        DomainError::PreconditionFailed {
            expected, current, ..
        } => {
            assert_eq!(expected, commit.revision() - 1);
            assert_eq!(current, commit.revision());
        }
        other => panic!("expected PreconditionFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn a_rejected_candidate_leaves_the_live_state_untouched() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    let other = test_effect_metadata("nebula");
    insert_effect(&state, &metadata).await;
    insert_effect(&state, &other).await;
    let layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };

    let mut stale = state.begin_scene_mutation().await;
    stale
        .upsert_primary_zone(&other, HashMap::new(), None, layout)
        .expect("candidate mutation should apply");

    apply_effect(&state, apply_command(metadata.id), MutationContext::api())
        .await
        .expect("the winning apply should succeed");

    let mut events = state.event_bus.subscribe_all();
    commit_scene(&state, stale)
        .await
        .expect_err("the stale candidate must be refused");

    // Nothing about the rejected candidate reached the world.
    let manager = state.scene_manager.read().await;
    let primary = manager
        .active_scene()
        .and_then(Scene::primary_group)
        .expect("the active scene should have a primary zone");
    assert_eq!(primary.effect_id, Some(metadata.id));
    assert!(
        events.try_recv().is_err(),
        "a rejected candidate publishes nothing"
    );
}

#[tokio::test]
async fn dropping_an_uncommitted_mutation_is_a_no_op() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;
    let layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };

    let revision_before = {
        let mut abandoned = state.begin_scene_mutation().await;
        abandoned
            .upsert_primary_zone(&metadata, HashMap::new(), None, layout)
            .expect("candidate mutation should apply");
        abandoned.base_revision()
    };

    // A later candidate still sees the original revision, so the drop
    // consumed no generation and moved no state.
    let next = state.begin_scene_mutation().await;
    assert_eq!(next.base_revision(), revision_before);
    let manager = state.scene_manager.read().await;
    assert!(
        manager
            .active_scene()
            .and_then(Scene::primary_group)
            .and_then(|zone| zone.effect_id)
            .is_none(),
        "an abandoned candidate never touches live state"
    );
}

/// A write that does not prove durable is not a rejection: the payload
/// stays the destination's newest admitted intent, the receipt says so,
/// and the announcement is withheld to match the failure the v1 wire
/// has always reported for one.
#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn a_non_durable_write_reports_retrying_and_publishes_nothing() {
    use hypercolor_daemon::persistence::AtomicFileWriter;

    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;

    // Destinations are keyed by canonical path, so a second writer for
    // the scenes file drives the same generation coordinator the store
    // uses.
    let scenes_path = state.data_dir.join("scenes.json");
    let writer = AtomicFileWriter::new(&scenes_path).expect("atomic writer");
    writer.set_injected_replace_failures(usize::MAX);

    let mut events = state.event_bus.subscribe_all();
    let applied = apply_effect(&state, apply_command(metadata.id), MutationContext::api())
        .await
        .expect("a non-durable write is not a rejection");

    assert_eq!(applied.commit.durability(), CommitDurability::Retrying);
    assert!(applied.commit.retry_error().is_some());
    assert!(
        events.try_recv().is_err(),
        "a commit whose write has not proven durable announces nothing"
    );

    writer.set_injected_replace_failures(0);
    writer.kick();
}

#[tokio::test]
async fn commit_generations_advance_the_scene_revision_in_order() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;

    let mut generations = Vec::new();
    for _ in 0..3 {
        let applied = apply_effect(&state, apply_command(metadata.id), MutationContext::api())
            .await
            .expect("apply should succeed");
        generations.push(applied.commit.generation());
        assert_eq!(applied.commit.generation(), applied.commit.revision());
    }

    assert!(
        generations.windows(2).all(|pair| pair[0] < pair[1]),
        "generations must be strictly increasing: {generations:?}"
    );
}
