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
use hypercolor_types::event::{HypercolorEvent, SceneLibraryChangeKind, ZoneChangeKind};
use hypercolor_types::layer::{
    LayerAdjust, LayerBlendMode, LayerSource, LayerTransform, MediaPlayback, SceneLayer,
    SceneLayerId,
};
use hypercolor_types::scene::{
    ColorInterpolation, EasingFunction, Scene, SceneId, SceneKind, SceneMutationMode,
    ScenePriority, SceneScope, TransitionSpec, UnassignedBehavior, ZoneRole,
};
use uuid::Uuid;

use hypercolor_daemon::api::AppState;
use hypercolor_daemon::domain::commit::CommitDurability;
use hypercolor_daemon::domain::effect::{ApplyEffect, RequestedTransition, apply_effect};
use hypercolor_daemon::domain::scene::{
    ActivateScene, CreateScene, UpdateScene, activate_scene, commit_scene, create_scene,
    deactivate_scene, delete_scene, update_scene,
};
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
        layout_id: None,
        activation_brightness: None,
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

fn apply_command(effect: &EffectMetadata) -> ApplyEffect {
    ApplyEffect {
        effect: effect.clone(),
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

    let applied = apply_effect(&state, apply_command(&metadata), MutationContext::api())
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

    apply_effect(&state, apply_command(&first), MutationContext::api())
        .await
        .expect("first apply should succeed");
    let applied = apply_effect(&state, apply_command(&second), MutationContext::api())
        .await
        .expect("second apply should succeed");

    let previous = applied
        .previous_effect
        .expect("the second apply should report the first effect");
    assert_eq!(previous.id, first.id.to_string());
    assert_eq!(applied.zone_change, ZoneChangeKind::Updated);
}

#[tokio::test]
async fn apply_effect_refuses_a_display_face() {
    let (state, _tempdir) = isolated_state();
    let mut metadata = test_effect_metadata("clock-face");
    metadata.category = EffectCategory::Display;
    insert_effect(&state, &metadata).await;

    let error = apply_effect(&state, apply_command(&metadata), MutationContext::api())
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
        let mut command = apply_command(&metadata);
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
            ..apply_command(&metadata)
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

    let error = apply_effect(&state, apply_command(&metadata), MutationContext::mcp())
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
    // Losing the commit compare-and-swap is a conflict, not a failed
    // caller precondition: no request carries a scene commit revision, so
    // a 412 here would be indistinguishable from the `If-Match` failures
    // the zone and layer routes serve.
    match error {
        DomainError::Conflict { details, .. } => {
            let details = details.expect("the conflict names the revisions");
            assert_eq!(details["kind"], "scene_commit_superseded");
            assert_eq!(details["expected_revision"], commit.revision() - 1);
            assert_eq!(details["current_revision"], commit.revision());
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
}

/// The temporary 2.3b bridge. Until every scene writer routes through
/// `commit_scene`, the revision cannot see a direct `scene_manager`
/// write, and the whole-manager install would discard it silently.
/// Divergence against the pristine base turns that into a retryable
/// conflict, and the direct write survives.
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

    apply_effect(&state, apply_command(&metadata), MutationContext::api())
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
    let applied = apply_effect(&state, apply_command(&metadata), MutationContext::api())
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
        let applied = apply_effect(&state, apply_command(&metadata), MutationContext::api())
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

// ── Scene library CRUD ───────────────────────────────────────────────────

fn create_command(name: &str) -> CreateScene {
    CreateScene {
        name: name.to_owned(),
        description: Some(format!("{name} scene")),
        enabled: None,
        mutation_mode: None,
        metadata: HashMap::new(),
    }
}

#[tokio::test]
async fn create_scene_seeds_a_default_zone_and_announces_the_scene() {
    let (state, _tempdir) = isolated_state();
    let mut events = state.event_bus.subscribe_all();

    let created = create_scene(&state, create_command("evening"), MutationContext::api())
        .await
        .expect("scene should be created");

    assert_eq!(created.scene.name, "evening");
    assert!(created.scene.enabled);
    assert_eq!(created.scene.mutation_mode, SceneMutationMode::Live);
    assert_eq!(
        created.scene.groups.len(),
        1,
        "every scene is born with a Default zone to select"
    );
    assert_eq!(created.scene.groups[0].role, ZoneRole::Primary);
    assert_eq!(created.commit.durability(), CommitDurability::Written);

    let manager = state.scene_manager.read().await;
    assert!(manager.get(&created.scene.id).is_some());
    drop(manager);

    assert_eq!(
        library_events(&mut events),
        vec![(created.scene.id, SceneLibraryChangeKind::Created)]
    );
}

/// MCP used to mint scenes with no zones and announce nothing, so a
/// scene created through the tool was unselectable in Studio and
/// invisible to every event-driven client until a refetch. One service
/// means one behavior.
#[tokio::test]
async fn create_scene_behaves_identically_for_both_transports() {
    let (state, _tempdir) = isolated_state();

    let via_api = create_scene(&state, create_command("api-made"), MutationContext::api())
        .await
        .expect("api create should succeed");
    let via_mcp = create_scene(&state, create_command("mcp-made"), MutationContext::mcp())
        .await
        .expect("mcp create should succeed");

    assert_eq!(via_api.scene.groups.len(), via_mcp.scene.groups.len());
    assert_eq!(via_api.scene.kind, via_mcp.scene.kind);
    assert_eq!(via_api.scene.priority, via_mcp.scene.priority);
    assert_eq!(
        via_api.scene.transition.duration_ms,
        via_mcp.scene.transition.duration_ms
    );
}

#[tokio::test]
async fn create_scene_carries_adapter_metadata_onto_the_scene() {
    let (state, _tempdir) = isolated_state();
    let mut command = create_command("triggered");
    command.metadata = HashMap::from([("trigger_type".to_owned(), "sunset".to_owned())]);
    command.mutation_mode = Some(SceneMutationMode::Snapshot);
    command.enabled = Some(false);

    let created = create_scene(&state, command, MutationContext::mcp())
        .await
        .expect("scene should be created");

    assert_eq!(
        created
            .scene
            .metadata
            .get("trigger_type")
            .map(String::as_str),
        Some("sunset")
    );
    assert_eq!(created.scene.mutation_mode, SceneMutationMode::Snapshot);
    assert!(!created.scene.enabled);
}

#[tokio::test]
async fn update_scene_rewrites_the_named_fields_and_keeps_the_rest() {
    let (state, _tempdir) = isolated_state();
    let created = create_scene(&state, create_command("evening"), MutationContext::api())
        .await
        .expect("scene should be created");
    let mut events = state.event_bus.subscribe_all();

    let updated = update_scene(
        &state,
        UpdateScene {
            scene_id: created.scene.id,
            name: "midnight".to_owned(),
            description: None,
            enabled: Some(false),
            mutation_mode: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("scene should update");

    assert_eq!(updated.scene.name, "midnight");
    assert!(!updated.scene.enabled);
    assert_eq!(updated.scene.mutation_mode, created.scene.mutation_mode);
    assert_eq!(
        updated.scene.groups.len(),
        created.scene.groups.len(),
        "an update must not drop the scene's zones"
    );
    assert_eq!(
        library_events(&mut events),
        vec![(created.scene.id, SceneLibraryChangeKind::Updated)]
    );
}

#[tokio::test]
async fn update_scene_refuses_an_unknown_scene() {
    let (state, _tempdir) = isolated_state();
    let error = update_scene(
        &state,
        UpdateScene {
            scene_id: SceneId::new(),
            name: "ghost".to_owned(),
            description: None,
            enabled: None,
            mutation_mode: None,
        },
        MutationContext::api(),
    )
    .await
    .expect_err("an unknown scene has nothing to update");
    assert!(
        matches!(error, DomainError::NotFound { .. }),
        "expected NotFound, got {error:?}"
    );
}

#[tokio::test]
async fn delete_scene_deactivates_it_and_announces_both_changes() {
    let (state, _tempdir) = isolated_state();
    let created = create_scene(&state, create_command("evening"), MutationContext::api())
        .await
        .expect("scene should be created");
    activate_scene(
        &state,
        ActivateScene {
            scene_id: created.scene.id,
            transition: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("scene should activate");
    let mut events = state.event_bus.subscribe_all();

    let deleted = delete_scene(&state, created.scene.id, MutationContext::api())
        .await
        .expect("scene should be deleted");

    assert_eq!(deleted.scene.id, created.scene.id);
    assert_eq!(deleted.previous_scene_id, Some(created.scene.id));
    assert_eq!(
        deleted.current_scene.as_ref().map(|scene| scene.id),
        Some(SceneId::DEFAULT),
        "deleting the active scene must fall back to Default"
    );

    let manager = state.scene_manager.read().await;
    assert!(manager.get(&created.scene.id).is_none());
    drop(manager);

    let seen = drain_events(&mut events);
    assert!(
        seen.iter()
            .any(|event| matches!(event, HypercolorEvent::ActiveSceneChanged { .. })),
        "the fallback to Default must be announced: {seen:?}"
    );
    assert!(
        seen.iter().any(|event| matches!(
            event,
            HypercolorEvent::SceneLibraryChanged {
                kind: SceneLibraryChangeKind::Deleted,
                ..
            }
        )),
        "the removal must be announced: {seen:?}"
    );
}

#[tokio::test]
async fn delete_scene_refuses_the_default_scene() {
    let (state, _tempdir) = isolated_state();
    let error = delete_scene(&state, SceneId::DEFAULT, MutationContext::api())
        .await
        .expect_err("the default scene is not deletable");
    assert!(
        matches!(error, DomainError::Conflict { .. }),
        "expected Conflict, got {error:?}"
    );
}

#[tokio::test]
async fn deactivate_scene_returns_to_default_and_reports_both_ends() {
    let (state, _tempdir) = isolated_state();
    let created = create_scene(&state, create_command("evening"), MutationContext::api())
        .await
        .expect("scene should be created");
    activate_scene(
        &state,
        ActivateScene {
            scene_id: created.scene.id,
            transition: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("scene should activate");

    let deactivated = deactivate_scene(&state, MutationContext::api())
        .await
        .expect("deactivation should succeed");

    assert_eq!(
        deactivated.previous_scene.map(|scene| scene.id),
        Some(created.scene.id)
    );
    assert_eq!(
        deactivated.current_scene.map(|scene| scene.id),
        Some(SceneId::DEFAULT)
    );
    let manager = state.scene_manager.read().await;
    assert_eq!(manager.active_scene_id().copied(), Some(SceneId::DEFAULT));
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

fn library_events(
    receiver: &mut tokio::sync::broadcast::Receiver<hypercolor_core::bus::TimestampedEvent>,
) -> Vec<(SceneId, SceneLibraryChangeKind)> {
    drain_events(receiver)
        .into_iter()
        .filter_map(|event| match event {
            HypercolorEvent::SceneLibraryChanged { scene_id, kind, .. } => Some((scene_id, kind)),
            _ => None,
        })
        .collect()
}

// ── The commit path is the only scene writer ─────────────────────────────

/// The revision compare-and-swap in `commit_scene` only sees commits: it
/// advances in `SceneCommitSequencer::admit`, which nothing but
/// `commit_scene` calls. A site that takes `scene_manager.write()`
/// directly therefore moves live scene state without moving the
/// revision, and because the commit installs a whole manager, such a
/// write landing inside a candidate's window is discarded with no error
/// to either caller. That is the lost-update class this wave closed, and
/// the fence is structural: reproducing it behaviorally would mean
/// racing a commit against a hand-rolled writer that no longer exists,
/// so the property is pinned against the sources instead.
///
/// Three writers are accounted for, each with a reason it cannot commit:
///
/// - `render_thread/scene_snapshot.rs` ticks transition progress every
///   frame. Committing it would mint a scene revision per frame and
///   invalidate every in-flight candidate, and progress is render-local
///   state no commit means to own. The reverse direction has a bounded
///   window — a candidate cloned before a tick and swapped in after it
///   rewinds progress by the clone-to-commit delta — which the next tick
///   re-advances, and which §6.1 closes by moving progress out of the
///   manager entirely.
/// - `scene_transactions.rs` publishes a prepared layout at the frame
///   boundary, holding the scene lock and the spatial lock together so
///   the renderer never sees a layout and a zone set that disagree.
///   `commit_scene` takes neither the spatial lock nor an `AppState` the
///   render thread can reach; Spec 76 §6.1 re-points commit at this
///   transaction rather than the reverse.
/// - `startup/lifecycle.rs` holds three, of different kinds. Two are
///   pre-init: both halves of the persisted-session restore run inside
///   `DaemonState::initialize`, before the render thread starts and
///   before the API binds. The third is post-teardown: shutdown's
///   deactivate runs after the render thread has been awaited out, so
///   the one cadence reader of scene state is already gone. None can
///   build an `AppState` — `from_daemon_state` requires a live input
///   publication pump, which exists in neither phase — and none has a
///   competing writer to order against.
///
/// A fourth entry means some new site can silently discard a commit —
/// or be discarded by one. Route it through `commit_scene` instead of
/// widening this list, unless it genuinely belongs to one of the three
/// reasons above.
///
/// This scan matches the receiver by name, so it fences the idiom the
/// tree actually uses rather than the type system. A future writer that
/// binds the handle to a differently named local slips past it, which is
/// why `the_scene_manager_handle_stays_where_it_is` pins the set of
/// modules that can reach the handle at all: gaining one is the review
/// event. The durable fence is Spec 76 §6.3's manager idiom, which moves
/// the lock inside a `SceneService` and makes the write lock
/// unreachable from outside the domain.
#[test]
fn no_scene_writer_lives_outside_the_commit_path() {
    /// Every file allowed to take the scene write lock, and how many
    /// times, in production code.
    const EXPECTED: [(&str, usize); 4] = [
        ("domain/scene.rs", 1),                 // commit_scene's install
        ("render_thread/scene_snapshot.rs", 1), // per-frame transition tick
        ("scene_transactions.rs", 1),           // frame-boundary layout publish
        ("startup/lifecycle.rs", 3),            // restore x2, shutdown deactivate
    ];

    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found: Vec<(String, usize)> = Vec::new();
    let mut pending = vec![src.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("source directory reads") {
            let path = entry.expect("source entry reads").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let relative = path
                .strip_prefix(&src)
                .expect("paths are under src")
                .to_string_lossy()
                .replace('\\', "/");
            // `#[cfg(test)] mod tests` is the tail of every file that has
            // one, and a test that drives the manager directly is not a
            // production writer. `scene_transactions/tests.rs` is such a
            // module in its own file.
            if relative.ends_with("/tests.rs") || relative == "tests.rs" {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("source file reads");
            let count = scene_write_lock_sites(&source);
            if count > 0 {
                found.push((relative, count));
            }
        }
    }
    found.sort();

    let mut expected = EXPECTED
        .iter()
        .map(|(file, count)| ((*file).to_owned(), *count))
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(
        found, expected,
        "every scene write lock outside commit_scene can silently discard a \
         concurrent commit; route the new site through commit_scene"
    );
}

/// Which modules can reach the scene handle at all.
///
/// A write lock has to come from somewhere, and every path to one starts
/// with an `Arc<RwLock<SceneManager>>` in scope. Nine modules name the
/// type; five of them only ever read or forward it, and pinning the set
/// means a tenth cannot appear without a reviewer seeing it — including
/// the case the sibling scan is blind to, where a new module binds the
/// handle to a local named anything at all.
///
/// Widening this list is a decision about who may touch scene state, not
/// a formality. Prefer handing the new module a domain service.
#[test]
fn the_scene_manager_handle_stays_where_it_is() {
    const EXPECTED: [&str; 9] = [
        "api/mod.rs",                  // AppState's field
        "discovery/mod.rs",            // reads the active scene's targets
        "interactive_preview.rs",      // reads zones for preview routing
        "network/host.rs",             // forwards the handle to drivers
        "render_thread.rs",            // render-thread state
        "scene_transactions.rs",       // frame-boundary layout publish
        "scene_transactions/tests.rs", // that transaction's own tests
        "startup/discovery_worker.rs", // forwards the handle
        "startup/mod.rs",              // DaemonState's field
    ];

    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    let mut pending = vec![src.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("source directory reads") {
            let path = entry.expect("source entry reads").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            if std::fs::read_to_string(&path)
                .expect("source file reads")
                .contains("RwLock<SceneManager>")
            {
                found.push(
                    path.strip_prefix(&src)
                        .expect("paths are under src")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    found.sort();

    let mut expected = EXPECTED.map(ToOwned::to_owned).to_vec();
    expected.sort();
    assert_eq!(
        found, expected,
        "a module that holds the scene handle can take the write lock under any local name,          which the write-site scan cannot see; hand it a domain service instead"
    );
}

/// Count `scene_manager.write()` acquisitions in one file's production
/// source, ignoring comments and tolerating the rustfmt line breaks that
/// split the receiver from the call.
///
/// `#[cfg(test)] mod` blocks are dropped first: a test that drives the
/// manager directly races nothing. They are dropped as blocks rather
/// than by truncating at the first one, because several files carry a
/// test module in the middle and a truncating scan would go blind to
/// every writer below it — which is exactly where `api/mod.rs` keeps
/// two. The block's end is the closing brace at the attribute's own
/// indentation, which rustfmt guarantees and `just fmt-check` enforces.
fn scene_write_lock_sites(source: &str) -> usize {
    let lines = source.lines().collect::<Vec<_>>();
    let mut production: Vec<&str> = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let is_test_module = line.trim() == "#[cfg(test)]"
            && lines
                .get(index + 1)
                .is_some_and(|next| next.trim_start().starts_with("mod "));
        if !is_test_module {
            production.push(line);
            index += 1;
            continue;
        }

        let declaration = lines[index + 1];
        if declaration.trim_end().ends_with(';') {
            // `mod tests;` — the block lives in its own file.
            index += 2;
            continue;
        }
        let indent = &line[..line.len() - line.trim_start().len()];
        let closing = format!("{indent}}}");
        index += 2;
        while index < lines.len() && lines[index] != closing {
            index += 1;
        }
        index += 1;
    }

    let code = production
        .iter()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    let collapsed = code.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.matches("scene_manager.write()").count()
        + collapsed.matches("scene_manager .write()").count()
}
